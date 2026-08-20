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
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{test::TermSize, viewport_to_point, Config, Term, TermMode};
use alacritty_terminal::tty;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};

use crate::osc_tap::{OscEvent, PromptMark, TapPty};
use crate::pty_loop::{Msg, Notifier, PtyLoop};
use crate::screen::{self, Screen};
use crate::theme::Theme;

/// PTY / IO スレッドからのイベント。UI 層はこれを `process_event` へ渡す
pub use alacritty_terminal::event::Event as TermEvent;

/// セッションが UI 層へ流すイベント（alacritty のイベント + OSC タップの検知）
#[derive(Debug)]
pub enum SessionEvent {
    /// alacritty_terminal の IO スレッドからのイベント
    Term(TermEvent),
    /// OSC 7 / 133 タップの検知（`osc_tap`。FR-2.4.1）
    Osc(OscEvent),
}

/// OSC 133 マークから導出するコマンド実行状態（FR-2.1.4 の表示・list の公開元）。
/// シェル統合が無いペインは Unknown のまま
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandState {
    /// シェル統合未検知（OSC 133 が一度も届いていない）
    #[default]
    Unknown,
    /// プロンプト表示中（入力待ち）
    Idle,
    /// コマンド実行中
    Running,
    /// 直近コマンドが非ゼロ exit で終了。次のコマンド実行開始まで保持する
    Failed(i32),
}

/// スクロールバックの保持行数
const SCROLLBACK_LINES: usize = 10_000;

/// シェルの既定 cwd（ホームディレクトリ）。macOS / Linux は `$HOME`、Windows は `%USERPROFILE%`。
/// 取得できなければ None（その場合は親プロセスの cwd を継承する alacritty の既定挙動になる）
fn default_home_dir() -> Option<PathBuf> {
    home_from(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

// 既定シェルの解決は抽象境界 B1（`platform::shell`）に閉じている。
// 呼び出し側は単一のコードパスを通る（`.agent/plans/2026-07-windows-port-architecture.md`）
pub(crate) use crate::platform::shell::default_shell;
pub use crate::platform::shell::login_shell_command;

/// ロケール既定注入の純粋ロジック（テスト用に env 参照と分離）。
/// LANG / LC_ALL / LC_CTYPE のどれも継承されないときだけ `LC_CTYPE=UTF-8` を返す
fn default_locale_env(
    lang: Option<std::ffi::OsString>,
    lc_all: Option<std::ffi::OsString>,
    lc_ctype: Option<std::ffi::OsString>,
) -> Option<(String, String)> {
    let unset = |v: &Option<std::ffi::OsString>| v.as_deref().is_none_or(|s| s.is_empty());
    (unset(&lang) && unset(&lc_all) && unset(&lc_ctype))
        .then(|| ("LC_CTYPE".to_string(), "UTF-8".to_string()))
}

/// `default_home_dir` の純粋ロジック（テスト用に env 参照と分離）。
/// `$HOME` を優先し、無ければ `%USERPROFILE%`。どちらも空なら None
fn home_from(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    home.or(userprofile)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

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
pub struct EventProxy(UnboundedSender<SessionEvent>);

impl EventListener for EventProxy {
    fn send_event(&self, event: TermEvent) {
        // 受信側（UI）が先に破棄されていても IO スレッドは落とさない
        let _ = self.0.unbounded_send(SessionEvent::Term(event));
    }
}

/// [`resize_plan`] の判断結果（#647）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizePlan {
    /// グリッドを作り直す（= reflow が起きる）か
    pub reflow_grid: bool,
    /// PTY へ winsize を通知するか
    pub notify_pty: bool,
}

/// リサイズで何をすべきかを決める純粋関数（#647）。
///
/// **cols/rows が同じでもセル寸法が変われば PTY へ通知する**のがこの関数の要点。
/// フォントサイズを変えたのに cols/rows が偶然一致する場面（ペイン単位ズーム中、
/// 端数の丸めで同数になる等）では、旧実装は早期 return してピクセル寸法
/// （`ws_xpixel` / `ws_ypixel`）を古いまま残していた。
///
/// 逆に、セル寸法だけ変わったときにグリッドを作り直してはいけない。
/// reflow は行構成を壊すので、必要が無いなら触らない
pub fn resize_plan(
    current_grid: (usize, usize),
    current_cell_px: (u16, u16),
    next_grid: (usize, usize),
    next_cell_px: (u16, u16),
) -> ResizePlan {
    let reflow_grid = next_grid != current_grid;
    let cell_changed = next_cell_px != current_cell_px;
    ResizePlan {
        reflow_grid,
        notify_pty: reflow_grid || cell_changed,
    }
}

/// 1 ペイン分のターミナルセッション（シェルプロセス + VT グリッド）
pub struct TerminalSession {
    term: Arc<FairMutex<Term<EventProxy>>>,
    notifier: Notifier,
    cols: usize,
    rows: usize,
    /// 直近に PTY へ通知したセル寸法（px）。cols/rows が同じでもここが変われば
    /// 通知し直す（#647。フォントサイズ変更で `ws_xpixel` が古いまま残るのを防ぐ）
    cell_px: (u16, u16),
    title: Option<String>,
    /// 起動時 working directory または OSC 7 で通知された cwd
    cwd: Option<PathBuf>,
    /// OSC 133 から導出したコマンド実行状態
    command_state: CommandState,
    /// command_state が最後に遷移した時刻（稼働時間表示用。#217）
    command_state_since: Option<std::time::Instant>,
    /// PTY スレーブの tty 名（tmux クライアントとの対応付け。FR-2.13.2）
    tty_name: Option<String>,
    /// PTY 直下の子プロセス（シェル / 明示コマンド）の pid。
    /// 器を持たないバックエンド（Windows の backend=none）で、ペイン配下の
    /// エージェント CLI をプロセス祖先辿りで見つけるための起点（#592）
    child_pid: Option<u32>,
    /// 検知された listen ポート（FR-2.4.2。UI 層のポーリングが更新する）
    listen_ports: Vec<crate::ports::ListenPort>,
    /// サブライン表示の下方向端数（0.0..1.0 行）。表示位置 = display_offset - fract。
    /// ピクセル単位スムーススクロール（#159）の描画専用状態で、グリッドには影響しない。
    /// ロック順序は fract → term に固定（逆順取得をしない）
    scroll_fract: std::sync::Mutex<f32>,
    /// 転送系ホイール（mouse reporting / alternate scroll）の行未満端数の持ち越し。
    /// トラックパッドの微小デルタを都度切り捨てると無反応になるため積分する
    wheel_carry: std::sync::Mutex<f32>,
    /// 転送系ホイールのレート制限状態（トークンバケット。#167）
    wheel_rate: std::sync::Mutex<WheelRateState>,
    /// 未処理の `Wakeup` があるか（#816。詳細は `pty_loop::PtyLoop::wakeup_pending`）
    wakeup_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 器（psmux）の copy mode 滞在の追跡と in-band 解除の仕込み（#686）
    copy_mode: std::sync::Mutex<CopyModeGate>,
}

/// 器（psmux）が copy mode（履歴閲覧）に居るあいだ打鍵を飲んでしまう問題への門番（#686）。
///
/// psmux はマウス要求のない内側アプリ（通常シェル）のペインでホイール報告を受けると
/// copy mode に入り、滞在中の打鍵を copy-mode コマンドとして解釈して**シェルへ渡さない**。
/// 実端末の作法は「スクロール中に打鍵したら最下部へ戻ってキーが通る」なので、
/// 打鍵の直前に解除キーを **同じ PTY へ前置**して器を抜けさせる。
///
/// 状態は 2 つ:
///
/// - `depth`: PTY へ転送したホイール報告の上下差。psmux は 1 報告 = 3 行を上下**対称**に
///   動かし、最下部に戻った時点で copy mode を抜ける（実測）。つまり `depth == 0` は
///   **器へ問い合わせずに**「copy mode ではない」と言い切れる
/// - `exit`: 前置する解除バイト列。**器へ問い合わせて copy mode だと確かめたときだけ**入る。
///   マウス要求 TUI（claude 等）のペインでは psmux は copy mode に入らず報告を内側アプリへ
///   転送するので、確かめずに撃つと TUI の入力欄へゴミ文字が入る
#[derive(Default)]
struct CopyModeGate {
    depth: i32,
    exit: Option<Vec<u8>>,
}

impl CopyModeGate {
    /// 器へ転送したホイール報告を記録する。正 = 過去方向（上）
    fn note_wheel(&mut self, lines: i32) {
        self.depth = self.depth.saturating_add(lines).max(0);
        if self.depth == 0 {
            // 最下部へ戻った = 器は copy mode を抜けている（実測）。
            // ここで降ろさないと「下まで戻してから打鍵」でゴミ文字が入る
            self.exit = None;
        }
    }

    /// 器の履歴を遡っている最中か（器へ問い合わせずに答えられる）
    fn scrolled_back(&self) -> bool {
        self.depth > 0
    }

    /// 解除を仕込む。**既に最下部へ戻っていれば何もしない**
    /// （器へ問い合わせた返事が届くまでの間にユーザーが下まで戻していた場合）
    fn arm(&mut self, bytes: &[u8]) {
        if self.scrolled_back() {
            self.exit = Some(bytes.to_vec());
        }
    }

    fn disarm(&mut self) {
        self.exit = None;
    }

    /// 仕込んである解除バイト列を取り出す（1 回だけ効く）。
    /// 解除が入れば器は最下部へ戻るので、遡り量の勘定も 0 に戻す
    fn take(&mut self) -> Option<Vec<u8>> {
        let taken = self.exit.take();
        if taken.is_some() {
            self.depth = 0;
        }
        taken
    }
}

/// ホイール転送レート制限（#167）の状態。tokens = 残イベント数、last = 最終補充時刻
struct WheelRateState {
    tokens: f32,
    last: std::time::Instant,
}

impl TerminalSession {
    /// シェル（または `options.command`）を PTY 上で起動する。
    /// 戻り値のレシーバが流すイベントは UI 層で `process_event` に渡すこと。
    /// セル寸法（px）は PTY の TIOCSWINSZ 用。UI 層が実測値で `resize` し直す前提の初期値
    pub fn spawn(
        cols: usize,
        rows: usize,
        options: SpawnOptions,
    ) -> Result<(Self, UnboundedReceiver<SessionEvent>), SessionError> {
        let (tx, rx) = unbounded::<SessionEvent>();
        let proxy = EventProxy(tx.clone());

        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            // kitty keyboard protocol（CSI > u の push/pop）を受理する。
            // 既定 false だと TUI の有効化要求が無視され Shift+Enter 等を区別できない
            kitty_keyboard: true,
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
        // TERM / COLORTERM はまずデフォルトを敷き、呼び出し側の env で上書きできるようにする。
        // alacritty_terminal の `setup_env` はホストプロセスの env を書き換える方式で tako は
        // 呼んでおらず、未設定だと親（.app は Finder 由来で TERM 不定）を継承して tmux 等が
        // 「missing or unsuitable terminal」で落ちる。alacritty terminfo は未導入環境が多いので
        // 安全側の xterm-256color を既定にし、24bit カラーは COLORTERM=truecolor で広告する。
        let mut env: std::collections::HashMap<String, String> = std::collections::HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ]);
        // ロケール未設定（Finder 起動の .app はプロセス環境に LANG が無い）だと、
        // ペイン内で起動した tmux クライアントが非 UTF-8 扱いになり CJK を `_` に
        // 置換する（2026-06-12 P0: 日本語全滅）。Terminal.app と同じく LC_CTYPE だけ
        // UTF-8 を既定注入する（メッセージ言語は変えない。継承 env / options.env が優先）
        if let Some((key, value)) = default_locale_env(
            std::env::var_os("LANG"),
            std::env::var_os("LC_ALL"),
            std::env::var_os("LC_CTYPE"),
        ) {
            env.insert(key, value);
        }
        // シェル統合（OSC 7/133 発行）の自動注入。options.env が常に優先
        env.extend(crate::shell_integration::env().iter().cloned());
        env.extend(options.env);

        // 明示コマンド（Claude 等）はシェル統合の OSC 7 を発行しないことがある。
        // PTY へ渡す起動 cwd をセッションにも保持し、相対パス解決やファイルツリーが
        // TUI 起動直後から同じ基準ディレクトリを使えるようにする。
        let working_directory = options.cwd.or_else(default_home_dir);
        let tty_options = tty::Options {
            // command 未指定なら既定シェルを明示解決する（login ラッパ回避。`default_shell`）
            shell: options
                .command
                .or_else(default_shell)
                .map(|c| tty::Shell::new(c.program, c.args)),
            // cwd 未指定なら親プロセスの cwd（.app 起動時は `/`）ではなくホームを既定にする。
            // 元ペインの cwd 継承は OSC 7 シェル統合（Phase 4）で対応する。
            working_directory: working_directory.clone(),
            env,
            ..tty::Options::default()
        };
        let mut pty = tty::new(&tty_options, window_size, 0).map_err(SessionError::Pty)?;
        // PTY スレーブの tty 名（/dev/ttysNNN）。tmux クライアントとの対応付けに使う（FR-2.13.2）
        let tty_name = slave_tty_name(&mut pty);
        // PTY 直下の子プロセス（シェル / 明示コマンド）の pid。
        // 器（tmux）を持たない環境で「このペインで何が動いているか」を辿る唯一の起点になる
        // （#592: Windows は backend=none なので tty / セッション名では対応付けられない）
        let child_pid = pty_child_pid(&pty);
        // 疑似コンソールの文字コードを UTF-8 に固定する（#655。Windows のみ実体を持つ）。
        // ConPTY は OEM コードページ（日本語版 Windows なら CP932）で始まるため、
        // 放っておくと子が吐いた UTF-8 バイトを conhost が CP932 として解釈し、
        // **tako が受け取る前に**文字が壊れる。上の `LC_CTYPE=UTF-8` 注入と同じ趣旨で、
        // tako が自分の前提（描画経路は UTF-8 専用）を自分で敷く。
        // 子が疑似コンソールへ接続し終えるまで数十 ms かかるので、待ちは別スレッドへ逃がす
        // （UI スレッドは止めない）。失敗してもペインは起動する
        if let Some(pid) = child_pid {
            crate::platform::console::pin_pane_to_utf8_when_ready(pid);
        }
        // PTY 読み取りを OSC 7 / 133 タップで観測する（バイト列は変更しない。`osc_tap`）
        let pty = TapPty::new(
            pty,
            Box::new(move |event| {
                let _ = tx.unbounded_send(SessionEvent::Osc(event));
            }),
        );

        // PTY IO ループは tako 側に持つ（`pty_loop`）。upstream の `EventLoop` は
        // reader スレッドのスタックへ 1 MiB を確保し、ペインごとに常駐していた（#817）
        let wakeup_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let event_loop = PtyLoop::new(term.clone(), proxy, pty, wakeup_pending.clone())
            .map_err(SessionError::EventLoop)?;
        let notifier = Notifier(event_loop.channel());
        let _io_thread = event_loop.spawn();

        Ok((
            Self {
                term,
                notifier,
                cols,
                rows,
                // spawn 時に PTY へ渡した初期セル寸法（上の `window_size` と一致させる）
                cell_px: (window_size.cell_width, window_size.cell_height),
                title: None,
                cwd: working_directory,
                command_state: CommandState::default(),
                command_state_since: None,
                tty_name,
                child_pid,
                listen_ports: Vec::new(),
                scroll_fract: std::sync::Mutex::new(0.0),
                wheel_carry: std::sync::Mutex::new(0.0),
                wheel_rate: std::sync::Mutex::new(WheelRateState {
                    tokens: WHEEL_FORWARD_BURST,
                    last: std::time::Instant::now(),
                }),
                copy_mode: std::sync::Mutex::new(CopyModeGate::default()),
                wakeup_pending,
            },
            rx,
        ))
    }

    /// PTY スレーブの tty 名（取得できない環境では None）
    pub fn tty_name(&self) -> Option<&str> {
        self.tty_name.as_deref()
    }

    /// PTY 直下の子プロセス（シェル / 明示コマンド）の pid（取得できない環境では None）。
    /// 起動時に確定した値で、シェルが exec で入れ替わっても pid 自体は変わらない。
    /// **プロセスの生存は保証しない**（終了後も残る）ので、生存前提の判定に使う側で確かめること
    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    /// tty 名の差し替え（Phase 5.5 tmux バックエンド用）。
    /// バックエンド構成ではペイン配下のプロセスは tmux サーバー側のペイン tty を
    /// 制御端末に持つため、ポート検知・tmuxview の突き合わせ先をそちらへ向ける
    pub fn set_tty_name(&mut self, tty: Option<String>) {
        self.tty_name = tty;
    }

    /// 現在のグリッドサイズ（cols, rows）
    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// OSC 0/2 で設定されたタイトル
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// グリッドと PTY（TIOCSWINSZ / ConPTY）の両方をリサイズする。セル寸法は px。
    ///
    /// cols/rows が同じでもセル寸法（`ws_xpixel` / `ws_ypixel`）が変われば PTY へ
    /// 通知し直す（#647）。フォントサイズを変えたのに cols/rows が偶然一致する
    /// 場面（ペイン単位ズーム中など）では、通知しないとピクセル寸法が古いまま残る
    pub fn resize(&mut self, cols: usize, rows: usize, cell_width: u16, cell_height: u16) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        let plan = resize_plan(
            (self.cols, self.rows),
            self.cell_px,
            (cols, rows),
            (cell_width, cell_height),
        );
        let ResizePlan {
            reflow_grid: grid_changed,
            notify_pty,
        } = plan;
        if !notify_pty {
            return;
        }
        if grid_changed {
            // リサイズは reflow で行構成が変わるため端数はリセット（整数位置へスナップ）
            *self.fract_lock() = 0.0;
            self.term.lock().resize(TermSize::new(cols, rows));
        }
        self.notifier.on_resize(WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width,
            cell_height,
        });
        self.cols = cols;
        self.rows = rows;
        self.cell_px = (cell_width, cell_height);
    }

    /// PTY（シェルの stdin）へバイト列を書き込む。
    /// キー入力時はスクロールバック表示を最下部へ戻す（一般的なターミナルの挙動）。
    ///
    /// 器（psmux）が copy mode に居ると分かっている場合は、**同じ書き込みの先頭に**
    /// 解除バイト列を混ぜる（#686）。実端末の作法「スクロール中に打鍵したら最下部へ
    /// 戻ってキーが通る」を、器へ別経路で命令せずに満たすためで、同じバイト列に
    /// 載せるので器が解除より先に打鍵を見ることが構造的に起こらない
    pub fn write(&self, bytes: Vec<u8>) {
        self.scroll_to_bottom();
        let bytes = match self.copy_mode_lock().take() {
            Some(mut prefix) => {
                prefix.extend_from_slice(&bytes);
                prefix
            }
            None => bytes,
        };
        self.notifier.notify(bytes);
    }

    /// 器の履歴を遡っている最中か（転送したホイールの上下差 > 0。#686）。
    /// **器へ問い合わせずに答えられる**ので、問い合わせるかどうかの門番に使う
    pub fn wheel_scrolled_back(&self) -> bool {
        self.copy_mode_lock().scrolled_back()
    }

    /// 次の打鍵へ copy mode の in-band 解除を仕込む（#686）。
    /// 器へ問い合わせて copy mode だと**確かめてから**呼ぶこと
    pub fn arm_copy_mode_exit(&self, bytes: &[u8]) {
        self.copy_mode_lock().arm(bytes);
    }

    /// copy mode 解除の仕込みを降ろす（器が「copy mode ではない」と答えた / 答えられない）
    pub fn disarm_copy_mode_exit(&self) {
        self.copy_mode_lock().disarm();
    }

    /// `copy_mode` のロック（毒化耐性は `fract_lock` と同じ理由）
    fn copy_mode_lock(&self) -> std::sync::MutexGuard<'_, CopyModeGate> {
        self.copy_mode
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// クリップボード文字列の貼り付け。アプリが要求していればブラケットペーストで包む
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        self.write(paste_payload(text, bracketed));
    }

    /// `scroll_fract` のロック。毒化しても継続する（描画専用の端数 f32 なので
    /// 壊れても実害がなく、パニック連鎖でセッションを失う方が重い）
    fn fract_lock(&self) -> std::sync::MutexGuard<'_, f32> {
        self.scroll_fract
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `wheel_carry` のロック（毒化耐性は `fract_lock` と同じ理由）
    fn carry_lock(&self) -> std::sync::MutexGuard<'_, f32> {
        self.wheel_carry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// スクロールバック表示を行数ぶん動かす（正で過去方向）。
    /// 行単位 API（CLI / MCP・キーボード）なので端数はリセットして整数位置へスナップする
    pub fn scroll_display(&self, delta_lines: i32) {
        let mut fract = self.fract_lock();
        *fract = 0.0;
        self.term.lock().scroll_display(Scroll::Delta(delta_lines));
    }

    /// スクロールバック表示を行の小数単位で動かす（正で過去方向）。
    /// ピクセル単位スムーススクロール（#159）の中核: 整数部は alacritty の
    /// display_offset、端数は `scroll_fract`（描画時のサブラインオフセット）に
    /// 分解して保持する。表示位置 pos = display_offset - fract（fract ∈ [0,1)）
    pub fn scroll_pixels(&self, delta_rows: f32) {
        let mut fract = self.fract_lock();
        let mut term = self.term.lock();
        let offset = term.grid().display_offset();
        let history = term.grid().history_size();
        let (delta_int, new_fract) = subline_scroll(offset, *fract, delta_rows, history);
        if delta_int != 0 {
            term.scroll_display(Scroll::Delta(delta_int));
        }
        *fract = new_fract;
    }

    /// 表示位置（行。0.0 = 最下部、増えると過去方向）。スクロールバーの位置計算用
    pub fn scroll_position(&self) -> f32 {
        let fract = *self.fract_lock();
        let offset = self.term.lock().grid().display_offset() as f32;
        (offset - fract).max(0.0)
    }

    /// サブライン表示の下方向端数（0.0..1.0 行）。描画側のピクセルシフト量と
    /// マウス座標→セル座標変換の補正に使う
    pub fn scroll_subline_fract(&self) -> f32 {
        *self.fract_lock()
    }

    /// 表示位置を行の小数で直接指定する（スクロールバードラッグ用）
    pub fn scroll_to_position(&self, pos: f32) {
        let current = self.scroll_position();
        self.scroll_pixels(pos - current);
    }

    /// マウスホイール入力。端末モードに応じて PTY 転送（mouse reporting /
    /// alternate scroll）と自前スクロールバック表示を出し分ける（`wheel_action`）。
    /// `col` / `row` は表示セル座標（mouse reporting の座標に使う）
    pub fn scroll_wheel(&self, delta_lines: i32, col: usize, row: usize) {
        let mode = *self.term.lock().mode();
        let delta_lines = if wheel_forwarded_to_pty(mode) {
            self.limit_forwarded_wheel(delta_lines)
        } else {
            delta_lines
        };
        match wheel_action(mode, delta_lines, col, row) {
            // 転送はスクロールバック表示を動かさない（write() の bottom 戻しも不要）
            WheelAction::Write(bytes) => {
                self.note_mouse_report(mode, delta_lines);
                self.notifier.notify(bytes)
            }
            WheelAction::ScrollDisplay(lines) => self.scroll_display(lines),
            WheelAction::None => {}
        }
    }

    /// マウス報告としてホイールを転送したときだけ遡り量を勘定する（#686）。
    /// alternate scroll（矢印キー代替）は内側アプリのスクロールで、
    /// 器の copy mode とは無関係なので数えない
    fn note_mouse_report(&self, mode: TermMode, delta_lines: i32) {
        if mode.intersects(TermMode::MOUSE_MODE) {
            self.copy_mode_lock().note_wheel(delta_lines);
        }
    }

    /// プログラム経由（CLI / MCP）のホイール送出（#687）。
    ///
    /// **ユーザーのホイールと同じバイト列を同じ PTY へ書く**が、慣性のレート制限
    /// （[`Self::limit_forwarded_wheel`]）は通さない。あれは trackpad の慣性で
    /// 無制限にイベントが流れ込むのを止めるためのもので、1 回の明示リクエストには
    /// 当てはまらない（8 イベントで頭打ちになると AI からのスクロールが動かない）。
    /// 代わりに [`PROGRAMMATIC_WHEEL_MAX`] で 1 リクエストの上限を切る:
    /// `wheel_action` はイベントを 1 本の buffer に畳んで**単一の write** にするので、
    /// #167 の「洪水の部分 write が escape-time を跨いで断片化する」経路には乗らない。
    ///
    /// 返り値は実際に送ったイベント数（符号は `delta_lines` と同じ）。
    /// **0 = このペインは転送経路ではない**（＝ 呼び出し側は表示スクロールを使うべき）
    pub fn send_wheel_report(&self, delta_lines: i32, col: usize, row: usize) -> i32 {
        let mode = *self.term.lock().mode();
        if !wheel_forwarded_to_pty(mode) {
            return 0;
        }
        let clamped = delta_lines.clamp(-PROGRAMMATIC_WHEEL_MAX, PROGRAMMATIC_WHEEL_MAX);
        match wheel_action(mode, clamped, col, row) {
            WheelAction::Write(bytes) => {
                // ユーザーのホイールと同じ扱いで遡り量を勘定する（#686）。
                // 器の copy mode 判定は「PTY へ転送した報告の上下差」なので、
                // 送出元が GUI か CLI かで数え方が変わってはいけない
                self.note_mouse_report(mode, clamped);
                self.notifier.notify(bytes);
                clamped
            }
            WheelAction::ScrollDisplay(_) | WheelAction::None => 0,
        }
    }

    /// マウスホイール入力の行小数版（GPUI のホイール / トラックパッドイベント用）。
    /// 表示スクロールは `scroll_pixels`（サブライン描画）、PTY 転送（mouse reporting /
    /// alternate scroll）は行未満を `wheel_carry` で積分して整数行だけ送る
    pub fn scroll_wheel_px(&self, delta_rows: f32, col: usize, row: usize) {
        let mode = *self.term.lock().mode();
        let forwards = mode.intersects(TermMode::MOUSE_MODE) || mode.contains(TermMode::ALT_SCREEN);
        if forwards {
            let mut carry = self.carry_lock();
            *carry += delta_rows;
            let lines = carry.trunc() as i32;
            *carry -= lines as f32;
            drop(carry);
            let lines = if wheel_forwarded_to_pty(mode) {
                self.limit_forwarded_wheel(lines)
            } else {
                lines
            };
            if lines != 0 {
                match wheel_action(mode, lines, col, row) {
                    WheelAction::Write(bytes) => {
                        self.note_mouse_report(mode, lines);
                        self.notifier.notify(bytes)
                    }
                    // ALT_SCREEN + alternate scroll OFF は何もしない（履歴が無い）
                    WheelAction::ScrollDisplay(_) | WheelAction::None => {}
                }
            }
        } else {
            *self.carry_lock() = 0.0;
            self.scroll_pixels(delta_rows);
        }
    }

    /// 転送系ホイール（PTY へ書く経路）のイベント数をトークンバケットで制限する（#167）。
    /// 慣性スクロールの洪水をそのまま PTY へ流すと、下流（tmux / 内側アプリ）の
    /// 読み取りが追いつかず macOS の tty 入力キューがバイトを黙って捨て、ESC を失った
    /// 断片（`4;45;18M` 等）が内側 TUI の入力欄へ平文として入る（実 claude で再現済み）。
    /// 超過イベントは捨てる（ホイールは相対量のため縮退しても壊れない。
    /// 表示スクロール（PTY に書かない経路）には適用しない）
    fn limit_forwarded_wheel(&self, delta_lines: i32) -> i32 {
        let now = std::time::Instant::now();
        let mut state = self
            .wheel_rate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let elapsed = now.duration_since(state.last).as_secs_f32();
        state.last = now;
        let (allowed, rest) = wheel_rate_take(state.tokens, elapsed, delta_lines.unsigned_abs());
        state.tokens = rest;
        if delta_lines < 0 {
            -(allowed as i32)
        } else {
            allowed as i32
        }
    }

    /// バックエンドペインの tmux 直接注入（`scroll_mirror::send_wheel`。#167）用に
    /// 転送レート制限だけを消費する。PTY へは書かない。
    /// send-keys のサブプロセス起動レートを抑える意味も兼ねる
    pub fn take_wheel_budget(&self, delta_lines: i32) -> i32 {
        self.limit_forwarded_wheel(delta_lines)
    }

    /// スクロールバック表示のオフセット（行。0 = 最下部）
    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    /// スクロールバックに保持している行数
    pub fn history_size(&self) -> usize {
        self.term.lock().grid().history_size()
    }

    /// スクロールバックの保持上限（ペインログの飽和判定用。Issue #112）
    pub fn scrollback_limit(&self) -> usize {
        SCROLLBACK_LINES
    }

    /// スクロールバック履歴の末尾から `skip_newest` 行飛ばして `count` 行を
    /// 平文（装飾なし・古い→新しい順）で返す。ペインログ（Issue #112）の増分取り込み用。
    /// 履歴が足りない分は取れた範囲だけ返す
    pub fn history_plain_lines(&self, skip_newest: usize, count: usize) -> Vec<String> {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags;

        let term = self.term.lock();
        let grid = term.grid();
        let history = grid.history_size();
        let available = history.saturating_sub(skip_newest);
        let take = count.min(available);
        if take == 0 {
            return Vec::new();
        }
        let cols = grid.columns();
        let mut out = Vec::with_capacity(take);
        // 履歴行は負の Line 番号（-1 = 最新の履歴行）。古い側から順に読む
        for offset in (skip_newest + 1..=skip_newest + take).rev() {
            let line = Line(-(offset as i32));
            let row = &grid[line];
            // #816: 行の大半は末尾の未使用セル（空白）で、`trim_end` で必ず落ちる。
            // 先に後ろから境界を探し、そこまでしか組み立てない（`String` の確保も
            // 1 本で済ませる）。取り出す文字列は従来と 1 バイトも変わらない
            let mut end = cols;
            while end > 0 {
                let cell = &row[Column(end - 1)];
                let spacer = cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
                if !spacer && cell.c != ' ' {
                    break;
                }
                end -= 1;
            }
            let mut text = String::with_capacity(end);
            for col in 0..end {
                let cell = &row[Column(col)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                text.push(cell.c);
            }
            // 空白以外の末尾空白類（タブ等）は従来どおり `trim_end` に任せる
            text.truncate(text.trim_end().len());
            out.push(text);
        }
        out
    }

    /// スクロールバック表示を絶対位置へ動かす（0 = 最下部。history を超えると先頭へクランプ）。
    /// 行単位 API なので端数はリセットして整数位置へスナップする
    pub fn scroll_to(&self, offset: usize) {
        let mut fract = self.fract_lock();
        *fract = 0.0;
        let mut term = self.term.lock();
        let current = term.grid().display_offset() as i32;
        let target = offset.min(term.grid().history_size()) as i32;
        if target != current {
            term.scroll_display(Scroll::Delta(target - current));
        }
    }

    /// alternate screen（全画面 TUI）中か。スクロールバーの表示判定等に使う
    pub fn is_alt_screen(&self) -> bool {
        self.term.lock().mode().contains(TermMode::ALT_SCREEN)
    }

    /// アプリが bracketed paste（`CSI ? 2004 h`）を要求しているか。
    ///
    /// これが偽のまま複数行を貼ると、`paste_payload` の改行正規化で 1 行ごとに CR が
    /// 入り、TUI 側では「行数ぶんの送信」になる（#623）。送達経路の診断で要る
    pub fn bracketed_paste(&self) -> bool {
        self.term.lock().mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// kitty keyboard protocol の disambiguate フラグ（TUI が `CSI > 1 u` で有効化）。
    /// 有効時、UI 層は Esc / 修飾付き Enter 等を CSI u 形式で送る（Shift+Enter の区別）
    pub fn disambiguate_keys(&self) -> bool {
        self.term
            .lock()
            .mode()
            .contains(TermMode::DISAMBIGUATE_ESC_CODES)
    }

    /// DECCKM（application cursor keys）が有効か。有効時、矢印キーは `ESC O A` 形式で
    /// 送る（`ESC [ A` ではない）。AI からのキー送出（#662）が
    /// [`crate::keys::KeyEncoding`] を組み立てるのに使う
    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(TermMode::APP_CURSOR)
    }

    /// このセッションへキーを送るときの符号化（#662）。
    /// TUI が要求したモードをそのまま反映する
    pub fn key_encoding(&self) -> crate::keys::KeyEncoding {
        crate::keys::KeyEncoding {
            app_cursor: self.app_cursor(),
            disambiguate: self.disambiguate_keys(),
        }
    }

    /// mouse reporting が要求されているか（ホイール転送の出し分けと同じ判定。
    /// tmux バックエンドの e2e 検証・デバッグ用）
    pub fn mouse_reporting(&self) -> bool {
        self.term.lock().mode().intersects(TermMode::MOUSE_MODE)
    }

    pub fn scroll_to_bottom(&self) {
        let mut fract = self.fract_lock();
        *fract = 0.0;
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
    pub fn process_event(&mut self, event: SessionEvent) -> Option<SessionNotice> {
        match event {
            SessionEvent::Term(event) => self.process_term_event(event),
            SessionEvent::Osc(event) => {
                self.process_osc_event(event);
                None
            }
        }
    }

    fn process_term_event(&mut self, event: TermEvent) -> Option<SessionNotice> {
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

    /// OSC 7 / 133 タップの検知を cwd・コマンド実行状態へ反映する（FR-2.4.1）
    fn process_osc_event(&mut self, event: OscEvent) {
        match event {
            OscEvent::CwdChanged(path) => self.cwd = Some(path),
            OscEvent::Mark(mark) => {
                let next = next_command_state(self.command_state, mark);
                if next != self.command_state {
                    self.command_state = next;
                    self.command_state_since = Some(std::time::Instant::now());
                }
            }
        }
    }

    /// 起動時 working directory または OSC 7 で通知された cwd
    pub fn cwd(&self) -> Option<&std::path::Path> {
        self.cwd.as_deref()
    }

    /// OSC 133 から導出したコマンド実行状態
    pub fn command_state(&self) -> CommandState {
        self.command_state
    }

    /// command_state が最後に遷移した時刻（稼働時間表示用。#217）。
    /// 一度も遷移していなければ None
    pub fn command_state_since(&self) -> Option<std::time::Instant> {
        self.command_state_since
    }

    /// 検知された listen ポート（FR-2.4.2。list / MCP に公開される）
    pub fn listen_ports(&self) -> &[crate::ports::ListenPort] {
        &self.listen_ports
    }

    /// listen ポート検知結果の反映。変化があれば true（再描画・通知の判断用）
    pub fn set_listen_ports(&mut self, ports: Vec<crate::ports::ListenPort>) -> bool {
        if self.listen_ports == ports {
            return false;
        }
        self.listen_ports = ports;
        true
    }

    /// 表示中グリッドの色解決済みスナップショット（描画・読み取りの基盤）。
    /// サブライン端数（`scroll_fract`）と部分表示用の追加行も含む
    pub fn screen(&self, theme: &Theme) -> Screen {
        self.screen_opts(theme, true)
    }

    /// カーソル強調を抑止できる版（tmux copy-mode スクロール中の描画用）
    pub fn screen_opts(&self, theme: &Theme, show_cursor: bool) -> Screen {
        let fract = *self.fract_lock();
        screen::snapshot_opts(&self.term.lock(), theme, show_cursor, fract)
    }

    /// 表示行を文字列で返す（装飾なし。セルフテスト・将来の `tako read` 用）
    pub fn visible_lines(&self) -> Vec<String> {
        self.screen(&Theme::default())
            .lines
            .into_iter()
            .map(|l| l.text.trim_end().to_string())
            .collect()
    }

    /// Claude TUI のフッターからエージェントメトリクスを抽出する。
    /// alt screen（TUI モード）のペインの末尾数行を走査し、
    /// `ctx NN%` や usage 情報をパースする。
    /// tmux バックエンド経由では alt screen フラグがホスト側に伝播しない
    /// ことがあるため、alt screen でなくても末尾行にパターンがあれば抽出する
    pub fn agent_metrics(&self) -> Option<AgentMetrics> {
        let lines = self.visible_lines();
        parse_agent_metrics(&lines)
    }

    /// 未処理 `Wakeup` フラグ（#816）。受け手はグリッドを読む直前にこれを倒す
    /// （`consume_wakeup`）。倒すまで PTY 側は次の `Wakeup` を送らないので、
    /// 1 read ごとにイベント配送タスクを起こすコストが消える
    pub fn wakeup_gate(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.wakeup_pending.clone()
    }

    /// Claude TUI の入力行（❯）のテキストがゴースト（自動提案）か手動入力かを分析する。
    /// screen snapshot のスタイルラン（dim フラグ）を検査して判定する
    pub fn analyze_input(&self) -> Option<screen::InputStatus> {
        let scr = self.screen(&Theme::default());
        screen::analyze_input_line(&scr)
    }
}

/// ステータスバーの利用制限表示で選択中のサービス（Issue #321）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LimitService {
    #[default]
    Claude,
    Codex,
    Agy,
}

impl LimitService {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "agy" => Some(Self::Agy),
            _ => None,
        }
    }

    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Agy];
}

/// TUI フッターから抽出したエージェントメトリクス（claude / codex 両対応。#357）
#[derive(Debug, Clone, Default)]
pub struct AgentMetrics {
    /// コンテキスト使用率（0–100）
    pub ctx_percent: Option<u32>,
    /// コンテキスト詳細テキスト（例: "128K/200K"）
    pub ctx_detail: Option<String>,
    /// usage 表示テキスト（例: "5h 23%", "$1.23" 等）
    pub usage_text: Option<String>,
    /// 5 時間リミット使用率（0–100。「5h NN%」表示から抽出。#217 ステータスバー）
    pub limit_5h: Option<u32>,
    /// 週リミット使用率（0–100。「7d NN%」「週 NN%」表示から抽出。#217）
    pub limit_week: Option<u32>,
    /// メトリクスの取得元（#357: サービス別にルーティングするため）
    pub source: MetricsSource,
    /// モデル表示名（例: "Opus 5"。フッターの `[Opus 5 (1M context) · xH]` から。#702）。
    /// `claude agents --json` は版によって `model` を返さないので、画面が実データの拠り所
    pub model: Option<String>,
}

/// メトリクスの取得元 CLI 種別（#357）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetricsSource {
    #[default]
    Unknown,
    Claude,
    Codex,
}

/// 画面行リストから Claude / Codex TUI フッターのメトリクスをパースする（#357 拡張）
fn parse_agent_metrics(lines: &[String]) -> Option<AgentMetrics> {
    // TUI のフッターは画面末尾 8 行以内にある（ステータスバー + ヒント行）
    let scan_lines: Vec<_> = lines.iter().rev().take(8).collect();
    let mut ctx_percent = None;
    let mut ctx_detail = None;
    let mut usage_text = None;
    let mut limit_5h = None;
    let mut limit_week = None;
    // codex 固有: `primary NN%` / `secondary NN%`（#357）
    let mut codex_primary = None;
    let mut codex_secondary = None;
    let mut source = MetricsSource::Unknown;
    let mut model = None;

    for line in &scan_lines {
        // --- Claude パターン ---
        // リミット表示（「5h NN%」「7d NN%」「週 NN%」。#217 ステータスバー）
        if limit_5h.is_none() {
            limit_5h = extract_labeled_percent(line, "5h");
        }
        if limit_week.is_none() {
            limit_week =
                extract_labeled_percent(line, "7d").or_else(|| extract_labeled_percent(line, "週"));
        }

        // --- Codex パターン（#357）---
        // `primary NN%` / `secondary NN%`（codex TUI フッターのレート制限表示）
        if codex_primary.is_none() {
            codex_primary = extract_labeled_percent(line, "primary");
        }
        if codex_secondary.is_none() {
            codex_secondary = extract_labeled_percent(line, "secondary");
        }

        // `ctx NN%` / `context NN%` / `Context NN% used` / `Context NN% left`
        if ctx_percent.is_none() {
            let after = line
                .to_ascii_lowercase()
                .find("ctx")
                .and_then(|pos| line.get(pos + 3..))
                .or_else(|| {
                    line.to_ascii_lowercase()
                        .find("context")
                        .and_then(|pos| line.get(pos + 7..))
                })
                .map(|s| s.to_string());
            if let Some(ref after) = after {
                if let Some(pct) = extract_percent(after) {
                    // codex は `Context NN% left` 表記（used = 100 - left）
                    let is_left = after.contains("left");
                    ctx_percent = Some(
                        if is_left {
                            100u32.saturating_sub(pct)
                        } else {
                            pct
                        }
                        .min(100),
                    );
                }
                if ctx_detail.is_none() {
                    ctx_detail = extract_ctx_detail(after);
                }
            }
        }

        // モデル名（#702。`[Opus 5 (1M context) · xH]` の角括弧セグメント）
        if model.is_none() {
            model = extract_model_name(line);
        }

        // usage パターン: `Nh NN%` / `Nm NN%` (時間残量) や `$N.NN` (コスト) やトークン数
        if usage_text.is_none() {
            if let Some(usage) = extract_usage_text(line) {
                usage_text = Some(usage);
            }
        }
    }

    // ソース判定: codex primary/secondary があれば Codex、5h/7d があれば Claude
    if codex_primary.is_some() || codex_secondary.is_some() {
        source = MetricsSource::Codex;
        // codex の primary/secondary を limit_5h/limit_week にマッピング
        // （UI は共通のメーター構造を使う）
        limit_5h = codex_primary;
        limit_week = codex_secondary;
    } else if limit_5h.is_some() || limit_week.is_some() {
        source = MetricsSource::Claude;
    }

    if ctx_percent.is_none() && usage_text.is_none() && limit_5h.is_none() && limit_week.is_none() {
        return None;
    }
    Some(AgentMetrics {
        ctx_percent,
        ctx_detail,
        usage_text,
        limit_5h,
        limit_week,
        source,
        model,
    })
}

/// フッターの角括弧からモデル表示名を取り出す（#702）。
///
/// 実測の形: `  [Opus 5 (1M context) · xH]  user@example.com`。
/// **既知のモデルファミリで始まるときだけ**採る — TUI の表示は版で変わるので、
/// 知らない文字列をモデル名としてヘッダに出すより、何も出さないほうが良い
fn extract_model_name(line: &str) -> Option<String> {
    let start = line.find('[')?;
    let rest = &line[start + 1..];
    let end = rest.find(']')?;
    // `·` 区切りの先頭要素がモデル、後続は effort 等
    let first = rest[..end].split('·').next()?.trim();
    // 括弧の補足（`(1M context)`）は落とす
    let name = first.split('(').next()?.trim();
    let known = ["Opus", "Sonnet", "Haiku", "Fable"];
    if name.is_empty() || !known.iter().any(|k| name.starts_with(k)) {
        return None;
    }
    Some(name.to_string())
}

/// 「<label> NN%」形式のパーセント値を抽出する（#217。ラベルの直前が英数字なら
/// 別トークンの一部とみなして飛ばす。例: "15h" の "5h" 誤マッチ防止）
fn extract_labeled_percent(line: &str, label: &str) -> Option<u32> {
    let mut search = 0;
    while let Some(rel) = line[search..].find(label) {
        let pos = search + rel;
        let before_ok = pos == 0
            || line[..pos]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
        if before_ok {
            if let Some(pct) = extract_percent(&line[pos + label.len()..]) {
                return Some(pct.min(100));
            }
        }
        search = pos + label.len();
    }
    None
}

/// `NNN.NK/NNNK` パターンの ctx 詳細テキストを抽出する（例: "128K/200K", "45.2K/200K"）
fn extract_ctx_detail(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    for i in 0..chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            let mut j = i + 1;
            // 数字 + ドットの列を読む
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                j += 1;
            }
            // K/M の単位
            if j < chars.len() && (chars[j] == 'K' || chars[j] == 'k' || chars[j] == 'M') {
                let unit_pos = j;
                j += 1;
                // / の後にもう一つの数字+単位
                if j < chars.len() && chars[j] == '/' {
                    j += 1;
                    let num2_start = j;
                    while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                        j += 1;
                    }
                    if j < chars.len()
                        && j > num2_start
                        && (chars[j] == 'K' || chars[j] == 'k' || chars[j] == 'M')
                    {
                        let text: String = chars[start..=j].iter().collect();
                        return Some(text);
                    }
                }
                // 単位だけ（K/M）で止まった場合は無視
                let _ = unit_pos;
            }
        }
    }
    None
}

/// 文字列から最初のパーセント値を抽出する（"  45%" → 45）
fn extract_percent(s: &str) -> Option<u32> {
    let mut num_start = None;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_digit() {
            if num_start.is_none() {
                num_start = Some(i);
            }
        } else if ch == '%' {
            if let Some(start) = num_start {
                return s[start..i].parse().ok();
            }
        } else if num_start.is_some() && !ch.is_ascii_digit() {
            // 数字列が途切れたら（バー文字等）リセット
            num_start = None;
        }
    }
    None
}

/// Claude TUI フッター行から usage テキストを抽出する。
/// パターン: `Nh NN%` / `$N.NN` / `NNNk tokens` / `NNN.Nk`
fn extract_usage_text(line: &str) -> Option<String> {
    // `Nh NN%` パターン（最も一般的な usage 表示）
    let chars: Vec<char> = line.chars().collect();
    for i in 0..chars.len() {
        // Nh パターン: 数字 + 'h' + 空白 + 数字 + '%'
        if chars[i].is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == 'h' || chars[j] == 'm') {
                let h_pos = j;
                j += 1;
                // 空白スキップ
                while j < chars.len() && chars[j] == ' ' {
                    j += 1;
                }
                let pct_start = j;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                if j < chars.len() && chars[j] == '%' && j > pct_start {
                    let text: String = chars[i..=j].iter().collect();
                    return Some(text);
                }
                // `Nh` だけでも有用
                if h_pos > i {
                    // もう少し先にコスト表示がある場合: `5h $1.23`
                    let mut k = h_pos + 1;
                    while k < chars.len() && chars[k] == ' ' {
                        k += 1;
                    }
                    if k < chars.len() && chars[k] == '$' {
                        let cost_start = k;
                        k += 1;
                        while k < chars.len() && (chars[k].is_ascii_digit() || chars[k] == '.') {
                            k += 1;
                        }
                        if k > cost_start + 1 {
                            let text: String = chars[i..k].iter().collect();
                            return Some(text);
                        }
                    }
                }
            }
        }

        // `$N.NN` コスト表示（単独）
        if chars[i] == '$' {
            let mut j = i + 1;
            let mut has_digit = false;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                if chars[j].is_ascii_digit() {
                    has_digit = true;
                }
                j += 1;
            }
            if has_digit && j > i + 1 {
                let text: String = chars[i..j].iter().collect();
                return Some(text);
            }
        }
    }
    None
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // IO スレッドへ終了を通知する（PTY が drop されシェルにも HUP が届く）
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

impl CommandState {
    /// 複数ペインの状態を「注目度」で集約する（タブの状態ドット・FR-2.10 集約センター用）。
    /// Failed > Running > Idle > Unknown
    pub fn aggregate<I: IntoIterator<Item = CommandState>>(states: I) -> CommandState {
        states.into_iter().fold(CommandState::Unknown, |acc, s| {
            if s.priority() > acc.priority() {
                s
            } else {
                acc
            }
        })
    }

    fn priority(self) -> u8 {
        match self {
            CommandState::Failed(_) => 3,
            CommandState::Running => 2,
            CommandState::Idle => 1,
            CommandState::Unknown => 0,
        }
    }
}

/// サブラインスクロールの位置計算（純関数）。
/// 表示位置 pos = offset - fract（0.0 = 最下部、増えると過去方向）に delta_rows を
/// 加算し、[0, history] へクランプして (display_offset の増減, 新しい fract) を返す。
/// display_offset = ceil(pos) / fract = ceil(pos) - pos ∈ [0, 1) の分解を保つ
fn subline_scroll(offset: usize, fract: f32, delta_rows: f32, history: usize) -> (i32, f32) {
    let pos = (offset as f32 - fract).max(0.0);
    let new_pos = (pos + delta_rows).clamp(0.0, history as f32);
    let new_offset = new_pos.ceil();
    (new_offset as i32 - offset as i32, new_offset - new_pos)
}

/// ホイール入力の出し分け先
#[derive(Debug, PartialEq, Eq)]
enum WheelAction {
    /// PTY へ書く（mouse reporting / alternate scroll）
    Write(Vec<u8>),
    /// 自前スクロールバック表示を動かす
    ScrollDisplay(i32),
    /// 何もしない
    None,
}

/// 転送系ホイールのレート上限（イベント/秒）と瞬間バースト上限（#167）。
/// レート 150/秒は「実 claude（tmux 越し・busy 中）で断片漏れゼロ」の実測値で、
/// ホイール高速回し（〜60 イベント/秒）には影響しない。
/// バーストは瞬間書き込みサイズを決める（8 イベント = 約 104 バイト）。
/// macOS の PTY 書き込みバッファ（1024B）に対し十分小さく保つことで、
/// 下流が詰まった際の部分 write がシーケンス途中で切れて escape-time を
/// 跨ぐ（= tmux が ESC を単独キー確定して残りが平文化する）事故を防ぐ
const WHEEL_FORWARD_RATE: f32 = 150.0;
const WHEEL_FORWARD_BURST: f32 = 8.0;

/// ホイール転送レート制限のトークン計算（純関数）。
/// tokens へ経過時間ぶんを補充（上限 `WHEEL_FORWARD_BURST`）し、
/// requested のうち通せるイベント数を返す。戻り値は（許可イベント数, 残トークン）
fn wheel_rate_take(tokens: f32, elapsed_secs: f32, requested: u32) -> (u32, f32) {
    let filled = (tokens + elapsed_secs.max(0.0) * WHEEL_FORWARD_RATE).min(WHEEL_FORWARD_BURST);
    let allowed = requested.min(filled as u32);
    (allowed, filled - allowed as f32)
}

/// マウスホイールレポート 1 イベント分のバイト列（SGR / X10 形式。up = 上方向）。
/// 直接ペインの PTY 転送（`wheel_action`）とバックエンドペインの tmux 直接注入
/// （`scroll_mirror::send_wheel`。#167）で共用する
pub fn wheel_report_bytes(sgr: bool, up: bool, col: usize, row: usize) -> Vec<u8> {
    // ホイールボタン: 64 = 上、65 = 下
    let button: u8 = if up { 64 } else { 65 };
    if sgr {
        format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes()
    } else {
        // X10 形式（各値 +32 の 1 バイト。座標は 223 が上限）
        vec![
            0x1b,
            b'[',
            b'M',
            32 + button,
            32 + (col + 1).min(223) as u8,
            32 + (row + 1).min(223) as u8,
        ]
    }
}

/// プログラム経由（CLI / MCP）のホイール送出で 1 リクエストに許す最大イベント数（#687）。
///
/// 1 イベント 6 バイト前後なので、上限でも 2KB 弱の**単一 write** にしかならない。
/// 器が 1 イベントあたり複数行スクロールする実装（tmux 既定は 3 行 / 5 行）でも、
/// 10000 行の履歴を数往復で走破できる大きさ
pub const PROGRAMMATIC_WHEEL_MAX: i32 = 300;

/// ホイールが PTY への書き込み（mouse reporting / alternate scroll の矢印変換）に
/// なるモードか。`wheel_action` が `Write` を返す条件と 1:1 に保つこと
fn wheel_forwarded_to_pty(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
        || (mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL))
}

/// ホイールの定石出し分け（alacritty / iTerm2 と同様）。`delta_lines` 正 = 上（過去）方向:
/// ① mouse reporting 中 → SGR / X10 のホイールボタンイベントを送る（TUI が自前処理）
/// ② alternate screen + alternate scroll（ESC[?1007、既定 ON）→ 上下矢印キーに変換
/// ③ それ以外の alternate screen → 何もしない（スクロールバックが無い）
/// ④ 通常画面 → 自前スクロールバック表示
fn wheel_action(mode: TermMode, delta_lines: i32, col: usize, row: usize) -> WheelAction {
    if delta_lines == 0 {
        return WheelAction::None;
    }
    let count = delta_lines.unsigned_abs() as usize;
    if mode.intersects(TermMode::MOUSE_MODE) {
        let event = wheel_report_bytes(
            mode.contains(TermMode::SGR_MOUSE),
            delta_lines > 0,
            col,
            row,
        );
        WheelAction::Write(event.repeat(count))
    } else if mode.contains(TermMode::ALT_SCREEN) {
        if mode.contains(TermMode::ALTERNATE_SCROLL) {
            let key: &[u8] = match (mode.contains(TermMode::APP_CURSOR), delta_lines > 0) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1bOB",
                (false, true) => b"\x1b[A",
                (false, false) => b"\x1b[B",
            };
            WheelAction::Write(key.repeat(count))
        } else {
            WheelAction::None
        }
    } else {
        WheelAction::ScrollDisplay(delta_lines)
    }
}

/// コマンド実行状態の遷移。エラー（Failed）はひと目で気づけるよう、
/// 次のコマンドが実行開始されるまでプロンプトに戻っても保持する（FR-2.1.4）
fn next_command_state(current: CommandState, mark: PromptMark) -> CommandState {
    match mark {
        PromptMark::PromptStart | PromptMark::CommandStart => match current {
            CommandState::Failed(code) => CommandState::Failed(code),
            _ => CommandState::Idle,
        },
        PromptMark::CommandExecuted => CommandState::Running,
        PromptMark::CommandFinished(Some(code)) if code != 0 => CommandState::Failed(code),
        PromptMark::CommandFinished(_) => CommandState::Idle,
    }
}

/// PTY master fd からスレーブの tty 名を得る（macOS: TIOCPTYGNAME）。
/// 失敗・未対応プラットフォームでは None（対応付け機能が劣化するだけで害は無い）
#[cfg(target_os = "macos")]
fn slave_tty_name(pty: &mut tty::Pty) -> Option<String> {
    use std::os::fd::AsRawFd;

    use alacritty_terminal::tty::EventedReadWrite;

    let fd = pty.reader().as_raw_fd();
    // TIOCPTYGNAME は 128 バイトのバッファを要求する
    let mut buf = [0u8; 128];
    let result = unsafe { libc::ioctl(fd, libc::TIOCPTYGNAME as _, buf.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0)?;
    std::str::from_utf8(&buf[..len]).ok().map(str::to_string)
}

#[cfg(not(target_os = "macos"))]
fn slave_tty_name(_pty: &mut tty::Pty) -> Option<String> {
    // Linux は ptsname_r、Windows は ConPTY で別概念。必要になったフェーズで対応する
    None
}

/// PTY 直下の子プロセスの pid（#592）。
/// alacritty_terminal は API がプラットフォームで分かれる:
/// unix は `Pty::child()`（`std::process::Child`）、Windows は
/// `Pty::child_watcher().pid()`（ConPTY 生成時の `GetProcessId`）。
/// 取得できなければ None（対応付けが劣化するだけで、既存経路には影響しない）
#[cfg(unix)]
fn pty_child_pid(pty: &tty::Pty) -> Option<u32> {
    Some(pty.child().id())
}

#[cfg(windows)]
fn pty_child_pid(pty: &tty::Pty) -> Option<u32> {
    pty.child_watcher().pid().map(|p| p.get())
}

#[cfg(not(any(unix, windows)))]
fn pty_child_pid(_pty: &tty::Pty) -> Option<u32> {
    None
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

    /// #816 の `Wakeup` ゲート: 未処理の `Wakeup` が残っている間は PTY 側が次を送らず、
    /// 受け手が倒すと再び送られる。これが崩れると「1 read ごとに配送タスクを起こす」
    /// 旧挙動（取り込み経路の 78%）に戻るか、逆に画面が止まる
    #[cfg(unix)]
    #[test]
    fn wakeupゲートは倒すまで次を送らない() {
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        // 20ms ごとに 1 行ずつ出す（1 行 = 1 read = 旧挙動なら 1 Wakeup）
        let script = "i=0; while [ $i -lt 60 ]; do printf 'g%d\\n' $i; sleep 0.02; \
                      i=$((i+1)); done";
        let (session, mut rx) = TerminalSession::spawn(
            40,
            8,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), script.to_string()],
                }),
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");
        let gate = session.wakeup_gate();
        let is_wakeup = |e: &SessionEvent| matches!(e, SessionEvent::Term(TermEvent::Wakeup));

        // 1 件目の Wakeup を待つ（= ゲートが立つ）
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut first = false;
        while Instant::now() < deadline && !first {
            match rx.try_recv() {
                Ok(ev) => first = is_wakeup(&ev),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        assert!(first, "最初の Wakeup が来ない");
        assert!(gate.load(Ordering::Acquire), "ゲートが立っていない");

        // 倒さずに待つ: 出力は続いているのに Wakeup は 1 件も増えない
        std::thread::sleep(Duration::from_millis(400));
        let mut extra = 0;
        while let Ok(ev) = rx.try_recv() {
            if is_wakeup(&ev) {
                extra += 1;
            }
        }
        assert_eq!(extra, 0, "倒す前に Wakeup が {extra} 件届いた");

        // 倒すと再び届く（= 画面が止まらない）
        gate.store(false, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut resumed = false;
        while Instant::now() < deadline && !resumed {
            match rx.try_recv() {
                Ok(ev) => resumed = is_wakeup(&ev),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        assert!(resumed, "ゲートを倒しても Wakeup が再開しない");
    }

    /// #816 で `history_plain_lines` は「後ろから境界を探して 1 本だけ組み立てる」形に
    /// なった。取り出す文字列は従来と 1 バイトも変わらないこと（末尾空白は落ちる /
    /// 全角のスペーサで欠けない / 行内の空白は残る）を実 PTY で固定する
    #[cfg(unix)]
    #[test]
    fn 履歴の平文行は末尾空白を落とし全角も欠けない() {
        use std::time::{Duration, Instant};

        // 20 桁 4 行に 12 行流すと、先頭の検査対象は履歴へ押し出される
        let script = "printf 'ab   \\nあい\\na b\\n'; \
                      i=0; while [ $i -lt 9 ]; do printf 'pad%d\\n' $i; i=$((i+1)); done";
        let (session, _rx) = TerminalSession::spawn(
            20,
            4,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), script.to_string()],
                }),
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");

        let deadline = Instant::now() + Duration::from_secs(15);
        while session.history_size() < 8 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let lines = session.history_plain_lines(0, 32);
        assert!(
            lines.iter().any(|l| l == "ab"),
            "末尾空白が落ちていない: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l == "あい"),
            "全角行が欠けた: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l == "a b"),
            "行内の空白が消えた: {lines:?}"
        );
        assert!(
            lines.iter().all(|l| !l.ends_with(' ')),
            "末尾空白が残っている: {lines:?}"
        );
    }

    /// #647: フォントサイズを変えると cols/rows は変わるので通知される。
    /// **cols/rows が偶然一致してもセル寸法が変われば通知する**のが本題
    #[test]
    fn リサイズ計画はセル寸法の変化だけでも_pty_へ通知する() {
        // 何も変わっていない = 何もしない（毎 render で呼ばれる経路なので必須）
        assert_eq!(
            resize_plan((80, 24), (8, 16), (80, 24), (8, 16)),
            ResizePlan {
                reflow_grid: false,
                notify_pty: false,
            }
        );
        // グリッドが変わった = reflow + 通知（従来どおり）
        assert_eq!(
            resize_plan((80, 24), (8, 16), (48, 11), (12, 26)),
            ResizePlan {
                reflow_grid: true,
                notify_pty: true,
            }
        );
        // グリッドは同じでセル寸法だけ変わった（フォントサイズ変更で cols/rows が
        // 偶然一致した場面）= reflow はしないが通知はする。旧実装はここで
        // 早期 return し ws_xpixel が古いまま残っていた
        assert_eq!(
            resize_plan((80, 24), (8, 16), (80, 24), (12, 26)),
            ResizePlan {
                reflow_grid: false,
                notify_pty: true,
            }
        );
        // 高さだけ変わる（行高だけ変えた場合）
        assert_eq!(
            resize_plan((80, 24), (8, 16), (80, 24), (8, 26)),
            ResizePlan {
                reflow_grid: false,
                notify_pty: true,
            }
        );
    }

    #[test]
    fn コマンド実行状態の遷移とエラー保持() {
        use CommandState::*;
        use PromptMark::*;
        // 通常サイクル: prompt → 実行 → 正常終了 → prompt
        assert_eq!(next_command_state(Unknown, PromptStart), Idle);
        assert_eq!(next_command_state(Idle, CommandExecuted), Running);
        assert_eq!(next_command_state(Running, CommandFinished(Some(0))), Idle);
        assert_eq!(next_command_state(Running, CommandFinished(None)), Idle);
        // 非ゼロ exit → Failed はプロンプトに戻っても保持し、次の実行開始で解除
        assert_eq!(
            next_command_state(Running, CommandFinished(Some(1))),
            Failed(1)
        );
        assert_eq!(next_command_state(Failed(1), PromptStart), Failed(1));
        assert_eq!(next_command_state(Failed(1), CommandStart), Failed(1));
        assert_eq!(next_command_state(Failed(1), CommandExecuted), Running);
    }

    /// **#686**: 器（psmux）が copy mode に居るあいだ打鍵が飲まれる問題の門番。
    /// 「確かめてから撃つ」「最下部へ戻ったら降ろす」「1 回だけ効く」を固定する
    #[test]
    fn copy_mode解除は確かめたときだけ仕込まれる() {
        let mut gate = CopyModeGate::default();
        // 最下部なら器へ聞くまでもなく copy mode ではない
        assert!(!gate.scrolled_back());
        // 確かめる前（= 遡ってもいない）に仕込もうとしても入らない
        gate.arm(b"q");
        assert_eq!(gate.take(), None, "確かめずに解除キーを撃ってはいけない");

        // 上へ遡る → 器へ問い合わせて copy mode と判明 → 打鍵に前置される
        gate.note_wheel(3);
        assert!(gate.scrolled_back());
        gate.arm(b"q");
        assert_eq!(gate.take(), Some(b"q".to_vec()));
        // 1 回だけ効く（解除後は器も最下部へ戻っている）
        assert_eq!(gate.take(), None);
        assert!(!gate.scrolled_back(), "解除後は遡り量も 0 に戻る");
    }

    /// **#686 の誤射防止**: 下まで戻せば器は copy mode を抜ける（実測）ので、
    /// 仕込みは器へ聞き直さずに降ろす。降ろさないとシェルへ `q` が入力される
    #[test]
    fn 最下部へ戻したら解除の仕込みを降ろす() {
        let mut gate = CopyModeGate::default();
        gate.note_wheel(5);
        gate.arm(b"q");
        gate.note_wheel(-2); // まだ遡り中
        assert!(gate.scrolled_back());
        gate.note_wheel(-3); // 最下部へ到達
        assert!(!gate.scrolled_back());
        assert_eq!(gate.take(), None, "最下部で解除キーを撃ってはいけない");
        // 行き過ぎても負にならない（器も最下部で止まる）
        gate.note_wheel(-10);
        assert!(!gate.scrolled_back());
        gate.note_wheel(1);
        assert!(gate.scrolled_back(), "1 報告で再び遡り中になる");
    }

    /// 器が「copy mode ではない」/「答えられない」と言ったら仕込みを降ろす。
    /// マウス要求 TUI（claude 等）のペインで `q` が入力欄へ入るのを防ぐ経路
    #[test]
    fn 器の否定で仕込みを降ろす() {
        let mut gate = CopyModeGate::default();
        gate.note_wheel(2);
        gate.arm(b"q");
        gate.disarm();
        assert_eq!(gate.take(), None);
        assert!(gate.scrolled_back(), "降ろしても遡り量の勘定は残る");
    }

    #[test]
    fn ホイールは端末モードで出し分ける() {
        let base = TermMode::default(); // ALTERNATE_SCROLL を含む
                                        // 通常画面 → 自前スクロールバック
        assert_eq!(wheel_action(base, 3, 0, 0), WheelAction::ScrollDisplay(3));
        // alternate screen + alternate scroll → 矢印キー × 行数
        let alt = base | TermMode::ALT_SCREEN;
        assert_eq!(
            wheel_action(alt, 2, 0, 0),
            WheelAction::Write(b"\x1b[A\x1b[A".to_vec())
        );
        assert_eq!(
            wheel_action(alt, -1, 0, 0),
            WheelAction::Write(b"\x1b[B".to_vec())
        );
        // app cursor モードでは SS3 形式
        assert_eq!(
            wheel_action(alt | TermMode::APP_CURSOR, 1, 0, 0),
            WheelAction::Write(b"\x1bOA".to_vec())
        );
        // alternate scroll が明示 OFF なら何もしない
        assert_eq!(
            wheel_action(alt - TermMode::ALTERNATE_SCROLL, 1, 0, 0),
            WheelAction::None
        );
        // mouse reporting（SGR）→ ホイールボタンイベント（座標は 1-based）
        let mouse = alt | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            wheel_action(mouse, 1, 4, 2), // col=4, row=2 → 5;3
            WheelAction::Write(b"\x1b[<64;5;3M".to_vec())
        );
        assert_eq!(
            wheel_action(mouse, -1, 0, 0),
            WheelAction::Write(b"\x1b[<65;1;1M".to_vec())
        );
        // mouse reporting（X10 レガシー）→ +32 バイト形式
        let x10 = base | TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            wheel_action(x10, 1, 0, 0),
            WheelAction::Write(vec![0x1b, b'[', b'M', 96, 33, 33])
        );
        // 0 行は無視
        assert_eq!(wheel_action(base, 0, 0, 0), WheelAction::None);
    }

    #[test]
    fn ホイール転送レート制限のトークン計算() {
        // バースト内は全部通る
        let (allowed, rest) = wheel_rate_take(WHEEL_FORWARD_BURST, 0.0, 3);
        assert_eq!(allowed, 3);
        assert!((rest - (WHEEL_FORWARD_BURST - 3.0)).abs() < 0.01);
        // 枯渇したら通らない（1 未満の端数では 1 イベントも許可しない）
        let (allowed, _) = wheel_rate_take(0.5, 0.0, 10);
        assert_eq!(allowed, 0);
        // 時間経過で補充される（150/秒 × 0.02 秒 = 3）
        let (allowed, _) = wheel_rate_take(0.0, 0.02, 100);
        assert_eq!(allowed, 3);
        // 補充はバースト上限まで（長時間放置しても溜め込まない）
        let (allowed, rest) = wheel_rate_take(0.0, 10.0, 1000);
        assert_eq!(allowed, WHEEL_FORWARD_BURST as u32);
        assert_eq!(rest, 0.0);
        // 慣性スクロール洪水（700 回 × 3 行を 0.07 秒で連打）の総転送量は
        // バースト + レート × 時間 に制限される（#167 の防御そのもの）
        let mut tokens = WHEEL_FORWARD_BURST;
        let mut total = 0;
        for _ in 0..700 {
            let (a, r) = wheel_rate_take(tokens, 0.0001, 3);
            total += a;
            tokens = r;
        }
        // 8 (burst) + 150/s × 0.07s ≈ 19
        assert!(total <= 19, "洪水は burst + rate 以内に制限される: {total}");
        assert!(total >= 8, "バーストぶんは即座に通る: {total}");
    }

    #[test]
    fn ホイール転送のpty書き込み判定はwheel_actionと一致する() {
        let base = TermMode::default();
        // 通常画面 = 表示スクロール（制限対象外）
        assert!(!wheel_forwarded_to_pty(base));
        // mouse reporting = PTY 転送
        assert!(wheel_forwarded_to_pty(base | TermMode::MOUSE_REPORT_CLICK));
        // alt screen + alternate scroll = 矢印キー変換（PTY 転送）
        assert!(wheel_forwarded_to_pty(base | TermMode::ALT_SCREEN));
        // alt screen で alternate scroll OFF = 何も書かない（制限対象外）
        assert!(!wheel_forwarded_to_pty(
            (base | TermMode::ALT_SCREEN) - TermMode::ALTERNATE_SCROLL
        ));
    }

    #[test]
    fn サブラインスクロールの位置計算() {
        // 最下部から半行遡る → offset 1 / fract 0.5（表示位置 = 1 - 0.5 = 0.5）
        assert_eq!(subline_scroll(0, 0.0, 0.5, 100), (1, 0.5));
        // さらに半行 → ちょうど 1 行（fract 0 に収束）
        assert_eq!(subline_scroll(1, 0.5, 0.5, 100), (0, 0.0));
        // 半行から戻す → 最下部へ（offset も fract も 0）
        assert_eq!(subline_scroll(1, 0.5, -0.5, 100), (-1, 0.0));
        // 最下部でさらに下 → 動かない（クランプ）
        assert_eq!(subline_scroll(0, 0.0, -5.0, 100), (0, 0.0));
        // 最古行でさらに上 → 動かない（クランプ）
        assert_eq!(subline_scroll(100, 0.0, 3.0, 100), (0, 0.0));
        // 履歴ゼロ（alt screen 等）では常に最下部のまま
        assert_eq!(subline_scroll(0, 0.0, 0.25, 0), (0, 0.0));
        // 整数行ぴったりのスクロールでは fract が発生しない
        assert_eq!(subline_scroll(0, 0.0, 3.0, 100), (3, 0.0));
        // 2.25 行遡り → offset 3 / fract 0.75（表示位置 2.25）
        let (d, f) = subline_scroll(0, 0.0, 2.25, 100);
        assert_eq!(d, 3);
        assert!((f - 0.75).abs() < 1e-5);
        // 微小デルタの積分: 0.1 行 × 10 回 ≒ 1 行（f32 誤差は 1e-4 以内）
        let (mut off, mut fr) = (0usize, 0.0f32);
        for _ in 0..10 {
            let (d, nf) = subline_scroll(off, fr, 0.1, 100);
            off = (off as i32 + d) as usize;
            fr = nf;
        }
        let pos = off as f32 - fr;
        assert!((pos - 1.0).abs() < 1e-4, "pos={pos}");
    }

    #[test]
    fn 状態の集約はfailedを最優先する() {
        use CommandState::*;
        assert_eq!(
            CommandState::aggregate([Idle, Running, Failed(2)]),
            Failed(2)
        );
        assert_eq!(CommandState::aggregate([Unknown, Idle, Running]), Running);
        assert_eq!(CommandState::aggregate([Unknown, Idle]), Idle);
        assert_eq!(CommandState::aggregate([]), Unknown);
    }

    #[test]
    fn ペースト改行は正規化されブラケットモードで包まれる() {
        assert_eq!(paste_payload("a\nb", false), b"a\rb".to_vec());
        assert_eq!(paste_payload("a\r\nb", false), b"a\rb".to_vec());
        assert_eq!(paste_payload("x", true), b"\x1b[200~x\x1b[201~".to_vec());
    }

    #[test]
    fn ロケール既定はどれも未設定のときだけ注入される() {
        use std::ffi::OsString;
        let utf8 = Some(("LC_CTYPE".to_string(), "UTF-8".to_string()));
        // 全部未設定 / 空 → 注入（.app の Finder 環境）
        assert_eq!(default_locale_env(None, None, None), utf8);
        assert_eq!(
            default_locale_env(Some(OsString::new()), None, Some(OsString::new())),
            utf8
        );
        // どれか 1 つでも設定済みなら触らない（ターミナル起動・ユーザー設定を尊重）
        assert_eq!(
            default_locale_env(Some(OsString::from("ja_JP.UTF-8")), None, None),
            None
        );
        assert_eq!(
            default_locale_env(None, Some(OsString::from("C")), None),
            None
        );
        assert_eq!(
            default_locale_env(None, None, Some(OsString::from("UTF-8"))),
            None
        );
    }

    #[test]
    #[allow(non_snake_case)]
    fn ホームディレクトリは_HOME_を優先し_空は無視する() {
        use std::ffi::OsString;
        // HOME があればそれを使う
        assert_eq!(
            home_from(
                Some(OsString::from("/Users/foo")),
                Some(OsString::from("C:\\u"))
            ),
            Some(PathBuf::from("/Users/foo"))
        );
        // HOME 無し → USERPROFILE（Windows）
        assert_eq!(
            home_from(None, Some(OsString::from("C:\\Users\\foo"))),
            Some(PathBuf::from("C:\\Users\\foo"))
        );
        // 空文字は無視（親 cwd 継承へフォールバック）
        assert_eq!(home_from(Some(OsString::new()), None), None);
        assert_eq!(home_from(None, None), None);
    }

    #[test]
    fn agent_metricsのctxパース() {
        // Claude TUI 典型フッター（プログレスバー文字を含む）
        let lines = vec![
            "some output".into(),
            "".into(),
            " ❯ ".into(),
            " Auto  5h 23%   ctx 45% ████░░░░░░  128K/200K".into(),
        ];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.ctx_percent, Some(45));
        assert_eq!(m.ctx_detail.as_deref(), Some("128K/200K"));
        assert_eq!(m.usage_text.as_deref(), Some("5h 23%"));
    }

    #[test]
    fn agent_metricsのctxのみ() {
        let lines = vec!["ctx 92%".into()];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.ctx_percent, Some(92));
    }

    #[test]
    fn agent_metricsのコスト表示() {
        let lines = vec!["  $1.23  ctx 50%".into()];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.ctx_percent, Some(50));
        assert_eq!(m.usage_text.as_deref(), Some("$1.23"));
    }

    #[test]
    fn agent_metricsのモデル名抽出() {
        // 実測フッター（claude 2.1.220）。GUI モードのチャットヘッダが使う（#702）
        let lines = vec![
            "  [Opus 5 (1M context) · xH]  user@example.com".into(),
            "  ctx   5% ░░░░░░░░░░".into(),
        ];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.model.as_deref(), Some("Opus 5"));
        assert_eq!(m.ctx_percent, Some(5));

        // 括弧無し・effort 無しでも拾える
        let lines = vec!["  [Sonnet 5]".into(), "ctx 10%".into()];
        assert_eq!(
            parse_agent_metrics(&lines).unwrap().model.as_deref(),
            Some("Sonnet 5")
        );
    }

    #[test]
    fn agent_metricsは知らない角括弧をモデル名にしない() {
        // 版差でフッターの中身が変わっても、知らない文字列をモデル名として出さない
        for line in ["  [tako-14]", "  [2026-08-01]", "  []", "  no brackets"] {
            let lines = vec![line.to_string(), "ctx 12%".into()];
            let m = parse_agent_metrics(&lines).unwrap();
            assert!(m.model.is_none(), "{line} をモデル名にしてはいけない");
        }
    }

    #[test]
    fn agent_metricsの該当なし() {
        let lines = vec!["normal shell output".into(), "$ ls".into()];
        assert!(parse_agent_metrics(&lines).is_none());
    }

    #[test]
    fn agent_metricsのリミット抽出() {
        // claudemeter 風ステータスライン（5h / 7d の 2 段 + 残り時間の括弧書き）
        let lines = vec![
            "5h  37% ████░░░░░░ (→2h34m)".into(),
            "7d  46% ████▓░░░░░ (→4d09h)".into(),
            "ctx 54% ████████░░".into(),
        ];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.limit_5h, Some(37));
        assert_eq!(m.limit_week, Some(46));
        assert_eq!(m.ctx_percent, Some(54));
        // 「週 NN%」表記でも取れる
        let lines = vec!["5h 62%  週 31%".into()];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.limit_5h, Some(62));
        assert_eq!(m.limit_week, Some(31));
        // 「15h」の 5h 誤マッチはしない
        let lines = vec!["uptime 15h 99%  ctx 10%".into()];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.limit_5h, None);
    }

    #[test]
    fn agent_metricsのalt_screen外でも検出() {
        // tmux バックエンド経由で alt screen フラグが伝播しないケース
        let lines = vec![
            "some output".into(),
            "  Auto  ctx 67% ████░░░  5h 12%".into(),
            " ❯ ".into(),
        ];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.ctx_percent, Some(67));
        assert_eq!(m.usage_text.as_deref(), Some("5h 12%"));
    }

    #[test]
    fn agent_metricsの分単位usage() {
        let lines = vec!["  45m 78%  ctx 30%".into()];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.ctx_percent, Some(30));
        assert_eq!(m.usage_text.as_deref(), Some("45m 78%"));
    }

    #[test]
    fn codexのprimary_secondaryパース() {
        // codex TUI 典型フッター（primary/secondary + Context）
        let lines = vec![
            "some output".into(),
            "".into(),
            " > ".into(),
            "primary 42%secondary 18%Context 67% used".into(),
        ];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.source, MetricsSource::Codex);
        assert_eq!(m.limit_5h, Some(42));
        assert_eq!(m.limit_week, Some(18));
        assert_eq!(m.ctx_percent, Some(67));
    }

    #[test]
    fn codexのcontext_left表記() {
        // codex は `Context NN% left` 表記（used ではなく残り）
        let lines = vec!["Context 30% left".into()];
        let m = parse_agent_metrics(&lines).unwrap();
        // 30% left → 70% used
        assert_eq!(m.ctx_percent, Some(70));
    }

    #[test]
    fn codexのprimary単独() {
        let lines = vec!["primary 85%".into()];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.source, MetricsSource::Codex);
        assert_eq!(m.limit_5h, Some(85));
        assert_eq!(m.limit_week, None);
    }

    #[test]
    fn claude_source判定() {
        let lines = vec!["5h 23%  7d 45%".into()];
        let m = parse_agent_metrics(&lines).unwrap();
        assert_eq!(m.source, MetricsSource::Claude);
        assert_eq!(m.limit_5h, Some(23));
        assert_eq!(m.limit_week, Some(45));
    }
}

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

use crate::limit_resume::{exhausted_limit_window, LimitWindow};
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

/// alacritty の `Term` 設定を組み立てる（Issue #818）。
///
/// tako が既定から変えるのは 2 つだけ。`scrolling_history` はペイン単位の
/// スクロールバック上限（正本は [`crate::scrollback`]）、`kitty_keyboard` は
/// CSI > u の push/pop 受理（既定 false だと Shift+Enter 等を区別できない）。
/// **[`TerminalSession::set_scrollback_limit`] が同じ関数で作り直す**ので、
/// 上限の動的変更で他の設定が巻き添えで既定へ戻ることはない
fn term_config(scrollback_lines: usize) -> Config {
    Config {
        scrolling_history: scrollback_lines,
        kitty_keyboard: true,
        ..Config::default()
    }
}

/// シェルの既定 cwd（ホームディレクトリ）。解決は [`crate::paths::home_dir`] 1 本に寄せている
/// （#870。ホーム解決を 2 か所に持つと片方だけ直る）。取得できなければ None
/// （その場合は親プロセスの cwd を継承する alacritty の既定挙動になる）
fn default_home_dir() -> Option<PathBuf> {
    crate::paths::home_dir()
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

/// ペインの端末申告（#946）。**親プロセスにも `options.env` にも依らない固定値**。
///
/// alacritty terminfo は未導入環境が多いので安全側の `xterm-256color` を名乗り、
/// 24bit カラーは `COLORTERM=truecolor` で広告する。
pub const PANE_TERMINAL_ENV: &[(&str, &str)] =
    &[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")];

/// PTY へ渡す env を仕上げる（#946）。
///
/// TERM / COLORTERM を**最後に上書き**するのが要点。既定として先に敷く形だと
/// 「注入する」という設計意図が呼び出し側の env 1 つで消えるうえ、消えたことは
/// どのログにも出ない。ここで上書きしておけば、`SpawnOptions::env` に何が入っていても、
/// tako-app を起こした親の端末が何であっても、ペインの申告は 1 つに決まる。
///
/// `inherit` は **#946 の注入口**（[`inherit_terminal_env`]）で、注入そのものを外して
/// 「親の TERM がペインへ素通しになる」壊れ方を同じバイナリで再現するためだけにある。
pub(crate) fn finalize_pane_env(
    mut env: std::collections::HashMap<String, String>,
    inherit: bool,
) -> std::collections::HashMap<String, String> {
    if inherit {
        // 注入を外す = 親プロセスの値がそのまま子へ継承される（alacritty は
        // `Command` の既定どおり親 env を引き継ぎ、指定ぶんだけ上書きするため）
        for (key, _) in PANE_TERMINAL_ENV {
            env.remove(*key);
        }
        return env;
    }
    for (key, value) in PANE_TERMINAL_ENV {
        env.insert((*key).to_string(), (*value).to_string());
    }
    env
}

/// **#946 の注入口**: `TAKO_946_INJECT=inherit` でペインへの TERM / COLORTERM 注入を外す。
///
/// 旧挙動の再現（`*_LEGACY`）ではない。旧実装も値は同じで、違いは「`options.env` で
/// 上書きできたか」だけなので、上書きする呼び出し側が居ない現状では観測できない。
/// 注入を外すアームにしてあるのは、**注入が効いているからこそ項目 1b が通る**ことと、
/// 継承が起きたときに診断が理由を名指しできることを、同じバイナリで示すため
fn inherit_terminal_env() -> bool {
    std::env::var("TAKO_946_INJECT").is_ok_and(|v| v == "inherit")
}

/// `TERMCHK=<TERM>,<COLORTERM>` の観測結果（#946 の診断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermCheck {
    /// 期待どおり注入されている
    Injected,
    /// tako-app を起こした親プロセスの TERM がそのままペインへ出ている
    Inherited { observed: String },
    /// 注入も継承も説明にならない値
    Unexpected { observed: String },
    /// 画面に `TERMCHK=` 行が無い（エコーが返っていない = 待ちや入力経路の問題）
    NoEcho,
}

impl TermCheck {
    /// 診断行の機械可読キー（grep 用）
    pub fn label(&self) -> &'static str {
        match self {
            Self::Injected => "injected",
            Self::Inherited { .. } => "inherited",
            Self::Unexpected { .. } => "unexpected",
            Self::NoEcho => "no-echo",
        }
    }

    /// 診断行に出す短い理由（日本語 1 行。**値そのものは端末名だけ**なので個人情報は乗らない）
    pub fn reason(&self) -> String {
        match self {
            Self::Injected => "ペインの TERM / COLORTERM は注入どおり".to_string(),
            Self::Inherited { observed } => format!(
                "親プロセスの TERM がペインへ継承されている（観測 {observed} / 期待 {}）。\
                 TERM 注入が効いていない = finalize_pane_env を通っていない疑い",
                expected_termchk()
            ),
            Self::Unexpected { observed } => format!(
                "ペインの TERM / COLORTERM が期待と違う（観測 {observed} / 期待 {}）",
                expected_termchk()
            ),
            Self::NoEcho => {
                "画面に TERMCHK= 行が無い（エコーが返っていない。待ち・入力経路の問題）".to_string()
            }
        }
    }
}

/// 項目 1b が期待する `TERMCHK=` の値
pub fn expected_termchk() -> String {
    PANE_TERMINAL_ENV
        .iter()
        .map(|(_, v)| *v)
        .collect::<Vec<_>>()
        .join(",")
}

/// 画面から拾った `TERMCHK=` 行を、親プロセスの申告と突き合わせて分類する（#946）。
///
/// **純関数**。画面テキストと親の TERM / COLORTERM だけを見るので、GUI を立てずに
/// 全分岐を単体テストできる。`screen` には画面末尾をそのまま渡してよい
/// （打ち込んだコマンド行そのものは `${TERM}` のままなので、展開後の行だけが拾われる）
pub fn diagnose_termchk(
    screen: &str,
    parent_term: Option<&str>,
    parent_colorterm: Option<&str>,
) -> TermCheck {
    let expected = expected_termchk();
    let Some(observed) = observed_termchk(screen) else {
        return TermCheck::NoEcho;
    };
    if observed == expected {
        return TermCheck::Injected;
    }
    // 親の申告（COLORTERM は無いことがあるので空文字で突き合わせる）と一致したら継承
    let parent = format!(
        "{},{}",
        parent_term.unwrap_or_default(),
        parent_colorterm.unwrap_or_default()
    );
    if observed == parent {
        return TermCheck::Inherited { observed };
    }
    // TERM だけが親と一致する形（COLORTERM は注入が効いた等）も継承として扱う。
    // 直したい相手は「TERM が親から来ている」ことで、COLORTERM の一致は必須ではない
    let observed_term = observed.split(',').next().unwrap_or_default();
    if !observed_term.is_empty() && Some(observed_term) == parent_term {
        return TermCheck::Inherited { observed };
    }
    TermCheck::Unexpected { observed }
}

/// 画面テキストから `TERMCHK=` の**展開後の値**を拾う（最後に出たものを採る）。
///
/// 入力はセルフテストの画面末尾（`pane_screen_tail`）をそのまま渡してよい。
/// あれは**行を ` | ` で連結した 1 行**なので、行区切りではなく
/// 「空白・`|`・引用符のどれかが来たら値の終わり」で切る（端末名にそれらは入らない）。
/// 打ち込んだコマンド行（`echo "TERMCHK=${TERM},${COLORTERM}"`）は展開前なので、
/// `$` や `{` を含む値は捨てる
pub fn observed_termchk(screen: &str) -> Option<String> {
    let mut found = None;
    for (idx, _) in screen.match_indices("TERMCHK=") {
        let rest = &screen[idx + "TERMCHK=".len()..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '|' | '"' | '\''))
            .unwrap_or(rest.len());
        let value = &rest[..end];
        if !value.is_empty() && !value.contains(['$', '{', '}']) {
            found = Some(value.to_string());
        }
    }
    found
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
    /// スクロールバックの保持上限（行。Issue #818）。None なら
    /// [`crate::scrollback::DEFAULT_LINES`]。器（tmux / psmux）越しのペインでも
    /// 外側 alacritty の履歴上限としてそのまま効く（`..options` で持ち回る）
    pub scrollback_lines: Option<usize>,
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
    /// PTY 直下の子プロセスの pid（#592。取得できない環境では None）
    child_pid: Option<u32>,
    /// スクロールバックの保持上限（行。Issue #818）。`Term` の設定と同じ値を
    /// 持つのは、上限を問い合わせるのに `Term` のロックを取りたくないため
    scrollback_lines: usize,
}

/// ホイール転送レート制限（#167）の状態。tokens = 残イベント数、last = 最終補充時刻
struct WheelRateState {
    tokens: f32,
    last: std::time::Instant,
}

/// 器（psmux）が copy mode（履歴閲覧）に居るあいだ打鍵を飲んでしまう問題への門番（#686）。
///
/// psmux はマウス要求のない内側アプリ（通常シェル）のペインでホイール報告を受けると
/// copy mode に入り、滞在中の打鍵を copy-mode コマンドとして解釈して**シェルへ渡さない**。
///
/// 解除を器へ別経路（`send-keys -X cancel`）で撃つと「解除が届く前に打鍵が
/// copy mode に食われる」競合が残るため、**同じ書き込みの先頭へ混ぜる**（in-band）。
/// 転送したホイールの上下差を tako 側で数えるので、**器へ問い合わせずに
/// 「いま遡っているか」を答えられる**（問い合わせるかどうかの門番になる）
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

        let scrollback_lines = crate::scrollback::clamp_lines(
            options
                .scrollback_lines
                .unwrap_or(crate::scrollback::DEFAULT_LINES),
        );
        let config = term_config(scrollback_lines);
        let term_size = TermSize::new(cols, rows);
        let term = Arc::new(FairMutex::new(Term::new(config, &term_size, proxy.clone())));

        let window_size = WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: 8,
            cell_height: 16,
        };
        // TERM / COLORTERM は**この関数の最後で無条件に上書きする**（#946。`finalize_pane_env`）。
        // alacritty_terminal の `setup_env` はホストプロセスの env を書き換える方式で tako は
        // 呼んでおらず、注入しないと親（.app は Finder 由来で TERM 不定 / 外部ターミナル起動なら
        // その端末の値）を継承して tmux 等が「missing or unsuitable terminal」で落ちる。
        let mut env: std::collections::HashMap<String, String> = std::collections::HashMap::new();
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
        // 使い捨ての検証（`cargo test` / セルフテスト / 隔離起動）では、シェルの履歴を
        // ユーザーの `~/.zsh_history` ではなく使い捨ての置き場へ向ける（#1253）。
        // ペインのシェルは対話ログインシェルなので、検証が打ち込んだコマンドが
        // そのまま本番の履歴に積もっていた（実測: #1200 のテストが 1 行残した）
        for (key, value) in crate::paths::verification_histfile_env() {
            env.insert(key, value);
        }
        // シェル統合（OSC 7/133 発行）の自動注入。options.env が常に優先
        env.extend(crate::shell_integration::env().iter().cloned());
        env.extend(options.env);
        // **TERM / COLORTERM だけは最後**（#946）。呼び出し側の env にも親プロセスにも
        // 依らせない = 「注入する」という設計意図が環境で消えない
        let env = finalize_pane_env(env, inherit_terminal_env());

        // 明示コマンド（Claude 等）はシェル統合の OSC 7 を発行しないことがある。
        // PTY へ渡す起動 cwd をセッションにも保持し、相対パス解決やファイルツリーが
        // TUI 起動直後から同じ基準ディレクトリを使えるようにする。
        let working_directory = options.cwd.or_else(default_home_dir);
        let mut tty_options = tty::Options {
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
        // argv を「1 語 = 1 引数」で子へ届ける（#884）。Windows は argv を 1 本の
        // コマンドラインへ組み直す必要があり、既定は素の空白連結なので空白を含む語
        // （`-c <空白入り cwd>` 等）が割れてペインが即死する。OS 差は境界の中だけが知る
        crate::platform::shell::apply_arg_escaping(&mut tty_options);
        let mut pty = tty::new(&tty_options, window_size, 0).map_err(SessionError::Pty)?;
        // PTY スレーブの tty 名（/dev/ttysNNN）。tmux クライアントとの対応付けに使う（FR-2.13.2）
        let tty_name = slave_tty_name(&mut pty);
        // 疑似コンソールの文字コードを UTF-8 に固定する（#655。Windows のみ実体を持つ）。
        // ConPTY は OEM コードページ（日本語版 Windows なら CP932）で始まるため、
        // 放っておくと子が吐いた UTF-8 バイトを conhost が CP932 として解釈し、
        // **tako が受け取る前に**文字が壊れる（描画経路は入口から出口まで UTF-8 専用）。
        // 子が疑似コンソールへ接続し終えるまで数十 ms かかるので、待ちは別スレッドへ
        // 逃がす（UI スレッドは止めない）。失敗してもペインは起動する
        let child_pid = pty_child_pid(&pty);
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
                listen_ports: Vec::new(),
                scroll_fract: std::sync::Mutex::new(0.0),
                wheel_carry: std::sync::Mutex::new(0.0),
                copy_mode: std::sync::Mutex::new(CopyModeGate::default()),
                child_pid,
                scrollback_lines,
                wheel_rate: std::sync::Mutex::new(WheelRateState {
                    tokens: WHEEL_FORWARD_BURST,
                    last: std::time::Instant::now(),
                }),
                wakeup_pending,
            },
            rx,
        ))
    }

    /// PTY スレーブの tty 名（取得できない環境では None）
    pub fn tty_name(&self) -> Option<&str> {
        self.tty_name.as_deref()
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
    /// 場面（ペイン単位ズームなど）では、通知しないとピクセル寸法が古いまま残る
    pub fn resize(&mut self, cols: usize, rows: usize, cell_width: u16, cell_height: u16) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        let ResizePlan {
            reflow_grid,
            notify_pty,
        } = resize_plan(
            (self.cols, self.rows),
            self.cell_px,
            (cols, rows),
            (cell_width, cell_height),
        );
        if !notify_pty {
            return;
        }
        if reflow_grid {
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

    /// PTY 直下の子プロセス（シェル / 明示コマンド）の pid（取得できない環境では None）。
    /// 起動時に確定した値で、シェルが exec で入れ替わっても pid 自体は変わらない。
    /// **プロセスの生存は保証しない**（終了後も残る）ので、生存前提の判定に使う側で確かめること
    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    /// 器の履歴を遡っている最中か（転送したホイールの上下差 > 0。#686）。
    /// **器へ問い合わせずに答えられる**ので、問い合わせるかどうかの門番に使う
    pub fn wheel_scrolled_back(&self) -> bool {
        self.copy_mode_lock().scrolled_back()
    }

    /// 次の書き込みの先頭へ混ぜる copy mode 解除バイト列を仕込む（#686）。
    /// 器へ問い合わせて copy mode と分かってから呼ぶ
    pub fn arm_copy_mode_exit(&self, bytes: &[u8]) {
        self.copy_mode_lock().arm(bytes);
    }

    /// 仕込んだ解除を降ろす（#686）
    pub fn disarm_copy_mode_exit(&self) {
        self.copy_mode_lock().disarm();
    }

    /// `copy_mode` のロック（毒化耐性は他のロックと同じ理由）
    fn copy_mode_lock(&self) -> std::sync::MutexGuard<'_, CopyModeGate> {
        self.copy_mode
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// マウス報告としてホイールを転送したときだけ遡り量を勘定する（#686）。
    /// alternate scroll（矢印キー代替）は内側アプリのスクロールで、
    /// 器の copy mode とは無関係なので数えない
    fn note_mouse_report(&self, mode: TermMode, delta_lines: i32) {
        if mode.intersects(TermMode::MOUSE_MODE) {
            self.copy_mode_lock().note_wheel(delta_lines);
        }
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
                self.notifier.notify(bytes);
            }
            WheelAction::ScrollDisplay(lines) => self.scroll_display(lines),
            WheelAction::None => {}
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
                        self.notifier.notify(bytes);
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

    /// スクロールバックの保持上限（ペインログの飽和判定用。Issue #112 / #818）
    pub fn scrollback_limit(&self) -> usize {
        self.scrollback_lines
    }

    /// スクロールバックの保持上限を変更する（Issue #818）。
    ///
    /// **既存ペインにもその場で効く**: alacritty の `set_options` は
    /// `Grid::update_history` を通り、下げたぶんの `Row`（= 桁数ぶんの
    /// `Vec<Cell>`）を実際に解放する。alt screen 中は非アクティブ側
    /// （= 履歴を持つ主グリッド）へ当たるので、TUI 表示中に変えても取りこぼさない。
    /// 範囲外の値は [`crate::scrollback::clamp_lines`] で丸める
    pub fn set_scrollback_limit(&mut self, lines: usize) {
        let lines = crate::scrollback::clamp_lines(lines);
        if lines == self.scrollback_lines {
            return;
        }
        self.scrollback_lines = lines;
        self.term.lock().set_options(term_config(lines));
    }

    /// スクロールバック履歴の末尾から `skip_newest` 行飛ばして `count` 行を
    /// 平文（装飾なし・古い→新しい順）で返す。ペインログ（Issue #112）の増分取り込み用。
    /// 履歴が足りない分は取れた範囲だけ返す
    pub fn history_plain_lines(&self, skip_newest: usize, count: usize) -> Vec<String> {
        use alacritty_terminal::index::Line;

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
        // 履歴行は負の Line 番号（-1 = 最新の履歴行）。古い側から順に読む。
        // **行を平文へ組むのは [`Self::compose_grid_row`] の 1 実装**（#1390）:
        // 行の由来が「可視グリッド」か「履歴」かの違いしかないのに末尾トリムを
        // 2 つ持つと、#1387（結合文字の欠落）のような「セルから文字を取り出す
        // 規則」の修正が片方だけに入って、ペインログ（#112）だけ黙って別の
        // 文字列になる。一致は `履歴の平文行は可視行と1バイトも変わらない` が見る
        for offset in (skip_newest + 1..=skip_newest + take).rev() {
            out.push(Self::compose_grid_row(&grid[Line(-(offset as i32))], cols));
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

    /// kitty keyboard protocol の disambiguate フラグ（TUI が `CSI > 1 u` で有効化）。
    /// 有効時、UI 層は Esc / 修飾付き Enter 等を CSI u 形式で送る（Shift+Enter の区別）
    pub fn disambiguate_keys(&self) -> bool {
        self.term
            .lock()
            .mode()
            .contains(TermMode::DISAMBIGUATE_ESC_CODES)
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

    /// 側路（[`crate::osc_sink`]）で運ばれてきた OSC バイト列を反映する（#766）。
    ///
    /// 器が OSC を素通ししない環境（psmux）では、統合スクリプトが**コンソールではなく
    /// ファイルへ**同じバイト列を書く。それをここへ流すと PTY 経路と**同じ**
    /// [`crate::osc_tap`] のスキャナと状態機械を通るので、cwd と実行状態の意味論が
    /// プラットフォーム間で分岐しない。
    ///
    /// スキャナは呼び出しごとに使い捨てる。側路のファイルは常に**完全な 1 束**を
    /// 持つ（書き手が rename で差し替える）ので、前回の途中状態を持ち越す必要が無い。
    /// 持ち越すと、上書きで消えた束の断片と次の束が繋がって化ける
    pub fn feed_osc_bytes(&mut self, bytes: &[u8]) {
        for event in crate::osc_tap::OscScanner::new().scan(bytes) {
            self.process_osc_event(event);
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

    /// ペインへ**セッションを借りずに**画面を読み・キーを書く手を取り出す（#1200）。
    ///
    /// tako-app が保持しているペインへの応答（選択肢ダイアログ）は、器が入力送出を
    /// 持たない環境（psmux）では**これが唯一の送り口**になる。`Send` なので
    /// バックグラウンドスレッド（数百 ms のスリープを挟む応答ループ）からも使える
    pub fn access(&self) -> PaneAccess {
        PaneAccess {
            term: self.term.clone(),
            notifier: Notifier(self.notifier.0.clone()),
        }
    }

    /// 表示行を文字列で返す（装飾なし。セルフテスト・将来の `tako read` 用）
    pub fn visible_lines(&self) -> Vec<String> {
        self.screen(&Theme::default())
            .lines
            .into_iter()
            .map(|l| l.text.trim_end().to_string())
            .collect()
    }

    /// 表示行の**末尾 `n` 行だけ**を平文で返す（#1301）。
    ///
    /// [`Self::visible_lines`] は [`Self::screen`]（= `snapshot_opts`）を通るので、
    /// 1 回ごとに `cols * rows` のセル配列を確保し、行ごとに `ScreenLine`
    /// （`String` + `Vec<StyleRun>` + `Vec<usize>` の 3 確保）を組んでから
    /// **その場で捨てて**文字列だけを取り出している。2 秒 tick の定期判定は
    /// どれも画面末尾しか見ないので、全ペインぶんの `Screen` を毎 tick
    /// 作り直す理由が無い（#1001 の H2 / H3）。
    ///
    /// 返す文字列は `visible_lines()` の末尾 `n` 行と**1 バイトも変わらない**
    /// （`n >= rows` なら全体。テスト `tail_lines_は_visible_lines_の末尾と一致する`）。
    /// 色・選択・カーソルはテキストを変えないので、装飾を解決せずに済む
    pub fn tail_lines(&self, n: usize) -> Vec<String> {
        use alacritty_terminal::index::Line;

        let term = self.term.lock();
        let rows = term.screen_lines();
        let take = n.min(rows);
        if take == 0 {
            return Vec::new();
        }
        let grid = term.grid();
        let cols = grid.columns();
        // display_offset d のビューポートは grid の Line(-d ..= rows-d-1)（`screen.rs` と同じ）。
        // その末尾 take 行を上から順に組む
        let first = rows as i32 - grid.display_offset() as i32 - take as i32;
        let mut out = Vec::with_capacity(take);
        for i in 0..take as i32 {
            out.push(Self::compose_grid_row(&grid[Line(first + i)], cols));
        }
        out
    }

    /// グリッドの 1 行を平文へ組む（[`Self::tail_lines`] /
    /// [`Self::visible_lines_filled`] / [`Self::history_plain_lines`] の共有部分）。
    ///
    /// 返す文字列は [`Self::visible_lines`]（= `screen::compose_line` + `trim_end`）と
    /// **1 バイトも変わらない**（テスト `tail_lines_は_visible_lines_の末尾と一致する` /
    /// `visible_lines_filled_は折り返しを右端で見分ける` /
    /// `履歴の平文行は可視行と1バイトも変わらない`）。
    ///
    /// **可視行と履歴行で実装を分けない**（#1390）: 行の由来が違うだけで
    /// 取り出す規則は同じなので、2 実装を持つと片方だけ直る
    fn compose_grid_row(
        row: &alacritty_terminal::grid::Row<alacritty_terminal::term::cell::Cell>,
        cols: usize,
    ) -> String {
        use alacritty_terminal::index::Column;

        // #816 と同じ作法: 行の大半は末尾の未使用セル（空白）で `trim_end` で必ず落ちる。
        // 先に後ろから境界を探し、そこまでしか組み立てない。
        // 「落とすセル」（全角の後続スペーサーと `\0`）と「0 幅の結合文字を積む」
        // 判断は `screen` の 1 実装を通すので、`visible_lines` と定義上一致する（#1387）
        let mut end = cols;
        while end > 0 && crate::screen::cell_is_trailing_blank(&row[Column(end - 1)]) {
            end -= 1;
        }
        let mut text = String::with_capacity(end);
        for col in 0..end {
            crate::screen::push_cell_text(&row[Column(col)], &mut text);
        }
        // 空白以外の末尾空白類（全角空白等）は `visible_lines` と同じく `trim_end` に任せる
        text.truncate(text.trim_end().len());
        text
    }

    /// 表示行と「その行が**右端まで埋まっているか**」を返す（#651）。
    ///
    /// 本文は [`Self::visible_lines`] と 1 バイトも変わらない。増えるのは 2 つ目の値で、
    /// **真なら次の行がこの行の折り返しの続きでありうる**ことを意味する
    /// （読む側は `tako_control::dispatch::find_exit_marker`）。
    ///
    /// # なぜ「埋まっているか」をグリッドから採るのか
    ///
    /// 画面テキストの**文字数**で代用すると、行に全角文字が 1 つあるだけで
    /// 「埋まっていない」と読む（`chars().count() < cols`）。終了マーカーの手前に全角の
    /// プロンプトが載る形（#325 の `続行しますか? (y/n): __TAKO_EXIT=1`）は実在するので、
    /// 判定は文字数ではなく**列**で持つ必要がある。alacritty の
    /// [`LineLength`](alacritty_terminal::term::cell::LineLength) は `WRAPLINE`
    /// （端末自身が折り返した印）が立っていれば全幅を返し、立っていなければ占有列数を返すので、
    ///
    /// - 直接 PTY のペイン: 折り返すのは alacritty 自身なので `WRAPLINE` が立つ
    /// - 器（tmux / psmux）のペイン: 折り返しは**器の中**で起きて、外側へは再描画として
    ///   届くので `WRAPLINE` が立つ保証が無い。占有列数が幅と一致することで拾う
    ///   （実測: 10 桁の tmux バックエンドのペインで割れたマーカーを拾えている）
    ///
    /// の両方を 1 つの物差しで見られる。**空白詰めの行を soft wrap 扱いにしてはいけない**
    /// （#1182 の実害: `Screen` の `cell_cols.last()` は空白詰めのせいで常に最終列を指すので、
    /// 画面全体が 1 本に連結された）ので、`Screen` 経由では判定しない
    ///
    /// # 既知の限界: 行末が全角のときは 1 列足りない（#1389）
    ///
    /// `line_length()` は**末尾から `cell.c != ' '` のセルを探す**実装なので、全角の
    /// 後続セル（`WIDE_CHAR_SPACER`）は `c == ' '` = **空きと数える**。だから
    /// **行末が全角の行は物理的に右端まで埋まっていても `filled=false` になる**
    /// （実測・10 桁: `abcdefghあ` で `line_length=9` / 右端セルは
    /// `Flags(WIDE_CHAR_SPACER)`）。端末自身が折り返した行だけは `WRAPLINE` が立って
    /// 全幅を返すので真になる = **救えないのは「行末が全角 + 以降の出力なし」と
    /// 「器が `CUP` で描き直した行」**で、上で名指しした器（tmux / psmux）の
    /// ペインこそここに当たる。
    ///
    /// 現行の消費者（`find_exit_marker`）はマーカーの断片を持つ行しか見ず、マーカーは
    /// 全 ASCII なのでその行の最終セルは必ず半角 = この取りこぼしは今の判定を
    /// 1 ビットも変えない。**この API を「折り返しの結合」一般へ使い回すなら右端の
    /// 全角を自分で検査する**こと（#1132 のとおり worker のペインは 21〜25 桁まで
    /// 狭まるので、狭いほど行末が全角になる確率は上がる）。限界は
    /// `visible_lines_filled_は行末の全角を取りこぼす` が固定し、兄弟実装
    /// `links::combined_screen_text` の同じ穴は `.agent/conventions.md` の
    /// #1283 節に並べてある
    pub fn visible_lines_filled(&self) -> Vec<(String, bool)> {
        use alacritty_terminal::index::Line;
        use alacritty_terminal::term::cell::LineLength;

        let term = self.term.lock();
        let rows = term.screen_lines();
        let grid = term.grid();
        let cols = grid.columns();
        // display_offset d のビューポートは grid の Line(-d ..= rows-d-1)（`screen.rs` と同じ）
        let first = -(grid.display_offset() as i32);
        (0..rows as i32)
            .map(|i| {
                let row = &grid[Line(first + i)];
                (
                    Self::compose_grid_row(row, cols),
                    row.line_length().0 >= cols,
                )
            })
            .collect()
    }

    /// Claude TUI のフッターからエージェントメトリクスを抽出する。
    /// alt screen（TUI モード）のペインの末尾数行を走査し、
    /// `ctx NN%` や usage 情報をパースする。
    /// tmux バックエンド経由では alt screen フラグがホスト側に伝播しない
    /// ことがあるため、alt screen でなくても末尾行にパターンがあれば抽出する。
    ///
    /// 採るのは画面末尾 [`AGENT_TUI_TAIL_LINES`] 行だけ（#1301）。
    /// `TAKO_1001_C2_LEGACY=1` で全画面スナップショットへ戻せる（A/B 用）
    pub fn agent_metrics(&self) -> Option<AgentMetrics> {
        let lines = if c2_legacy() {
            self.visible_lines()
        } else {
            self.tail_lines(AGENT_TUI_TAIL_LINES)
        };
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

/// 2 秒 tick の定期判定が画面から採る**末尾の行数**（#1301）。
///
/// この窓の**正本はここ 1 箇所**で、採る側（`TerminalSession::agent_metrics` /
/// `main.rs` の `drive_queued_message_recovery`）と読む側（[`parse_agent_metrics`] /
/// `claude_tui::queued_messages_pending`）が同じ値を引く。片方だけ広げると
/// 「画面には出ているのに読めない」状態になる。
///
/// 値の根拠（読む側が要る窓の広いほう + 余裕）:
///
/// - フッター（[`FOOTER_SCAN_LINES`] = 8 物理行）: `ctx NN%` / `5h NN%` / `7d NN%` / モデル名
/// - 上限の見出し（[`EXHAUSTED_SCAN_LINES`] = 24 物理行、外したら **24 論理行**で再走査）:
///   実採取（25 桁の狭いペイン・#1123）で見出しは画面末尾から **16 物理行**目に居た。
///   48 はそれに 3 倍、物理 24 行の窓には 2 倍の余裕がある
/// - 入力欄（`claude_tui::bottom_prompt_content` は最下部のプロンプト行）:
///   実測でフッター 6 行 + 空行 1 + 枠 3 行 = 末尾から 9〜10 行目（#1093）
///
/// **rows がこの値以下のペインでは全画面と一致する**（= 大半のペインで判定は 1 ビットも動かない）。
/// 節約の主体は行数ではなく `Screen`（`ScreenLine` の 3 確保 / 行）を組まないこと
pub const AGENT_TUI_TAIL_LINES: usize = 48;

/// 窓の正本が、読む側が要る窓を両方とも覆っていることを**コンパイル時**に固定する（#1301）。
/// 片方の窓を広げたらここでビルドが落ちる = 採る側も一緒に直る
/// （テストではなくビルドで止めるので、`--lib` を走らせない経路でも取りこぼさない）
const _: () = assert!(
    AGENT_TUI_TAIL_LINES >= FOOTER_SCAN_LINES,
    "フッターの走査窓が AGENT_TUI_TAIL_LINES を超えている（採る窓を広げること。#1301）"
);
const _: () = assert!(
    AGENT_TUI_TAIL_LINES >= EXHAUSTED_SCAN_LINES,
    "上限見出しの走査窓が AGENT_TUI_TAIL_LINES を超えている（採る窓を広げること。#1093 / #1123 / #1301）"
);

/// `TAKO_1001_C2_LEGACY=1` で #1301 前（全ペインのフルスナップショット + alt screen ゲート無し）
/// へ戻す。同一バイナリで A/B を取る入口（`main.rs` の `refresh_agent_metrics` も同じ関数を引く）
pub fn c2_legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1001_C2_LEGACY").is_some())
}

/// TUI フッター（ステータスバー + ヒント行）を走査する画面末尾の行数
const FOOTER_SCAN_LINES: usize = 8;

/// 画面行リストからメトリクスを抽出する（#1021）。
///
/// `TerminalSession::agent_metrics` は**生きたセッション**を要るので GUI 層しか呼べない。
/// dispatch（CLI / MCP）も同じ判定を通せるように、行の列だけを受ける入口を公開する
/// （ctx% の解決を 4 経路が同じ 1 実装で行うため）
pub fn agent_metrics_from_lines(lines: &[String]) -> Option<AgentMetrics> {
    parse_agent_metrics(lines)
}

/// 画面テキストからメトリクスを抽出する（#1021。dispatch は 1 本の文字列で持っている）
pub fn agent_metrics_from_text(text: &str) -> Option<AgentMetrics> {
    let lines: Vec<String> = text.lines().map(|l| l.trim_end().to_string()).collect();
    parse_agent_metrics(&lines)
}

/// 画面行リストから Claude / Codex TUI フッターのメトリクスをパースする（#357 拡張）
fn parse_agent_metrics(lines: &[String]) -> Option<AgentMetrics> {
    // TUI のフッターは画面末尾 8 行以内にある（ステータスバー + ヒント行）
    let scan_lines: Vec<_> = lines.iter().rev().take(FOOTER_SCAN_LINES).collect();
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

        // claude の**組み込み**表示（#1021。ユーザーが statusLine を設定していない
        // ときに出る 3 形式）。`ctx`/`context` の後ろから % を探す下の経路より**先**に
        // 見る: `Context low (12% remaining)` を素直に読むと「12% 使用」= 残量と
        // 使用率を取り違える（実際は 88% 使用）
        if ctx_percent.is_none() {
            ctx_percent = builtin_ctx_percent(line);
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

    // #1093: 上限に達した見出しが画面に出ていて、**フッターが数値を出していない**なら、
    // その枠は使い切っている（= 100%）。旧実装はフッターの `5h NN%` だけを見ていたので、
    // 上限で止まった瞬間にメーターが `--`（データ無し）へ落ちていた
    // ——「いま上限中」であることが画面に書いてあるのに UI からは分からない状態だった。
    //
    // 誤って 100% に貼り付かないための二重の条件:
    //  ① フッターが数値を出しているときは触らない（実データが正）。実測した稼働中の
    //     claude ペイン 3 枚はいずれも末尾 6 行に `ctx NN%` / `5h NN%` / `7d NN%` を
    //     そろえて出していたので、ここへ落ちてくるのは通常**フッターが黙っている =
    //     上限中**のときだけになる。復帰してフッターが数値を出し始めれば実データが勝つ
    //  ② 枠の名前が読めた見出しだけを採る（`exhausted_limit_window`）。
    //     `usage credit limit` のように 5h / 週へ対応づけられないものは `--` のまま
    //
    // **未検証**: 「上限中はフッターが `5h NN%` を出さなくなる」ことは上限を実際に
    // 踏まないと確かめられない。根拠は #1093 の実地報告（3 worker が止まっている
    // あいだステータスバーが `--` = 全ペインで `limit_5h` が None だった）で、
    // フッターが数値を出す構成なら ① によりこの分岐は素通りする
    if limit_5h.is_none() || limit_week.is_none() {
        if let Some(window) = exhausted_limit_window_in(lines) {
            match window {
                LimitWindow::FiveHour => limit_5h = limit_5h.or(Some(100)),
                LimitWindow::Week => limit_week = limit_week.or(Some(100)),
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

/// 上限に達した見出しを探す走査範囲（画面末尾からの行数。#1093）。
///
/// 見出しは会話の最後の出力なので、**入力欄とフッターのぶんだけ上**にある。
/// 実測（claude 2.1.258 の稼働中ペイン）ではフッターが 6 行・入力欄が 3 行・
/// あいだの空行が 1 行で少なくとも 10 行、生成中のスピナー行が残っていれば 13〜15 行。
/// フッターだけを見る 8 行の窓（`parse_agent_metrics` 本体）では届かないので
/// これだけ別の窓を持つ。実測の 15 行に余裕を足した値で、走査対象は可視画面
/// （`visible_lines`）なので深いスクロールバックまでは遡らない
const EXHAUSTED_SCAN_LINES: usize = 24;

/// 画面末尾から上限の見出しを探す（下の行ほど新しいので末尾から見る。#1093）。
///
/// 物理行で外れたら**折り返しを結合した論理行**でもう一度見る（#1123）。狭いペイン
/// （実発生は 21〜25 桁）では見出しが 3〜4 行に割れて 1 本も規則に当たらず、
/// 自動復帰が発火しないのと同時にメーターも `--` のままになっていた。
/// 物理行を先に見るので、折り返しの無い画面では判定が 1 ビットも変わらない
fn exhausted_limit_window_in(lines: &[String]) -> Option<LimitWindow> {
    lines
        .iter()
        .rev()
        .take(EXHAUSTED_SCAN_LINES)
        .find_map(|l| exhausted_limit_window(l))
        .or_else(|| {
            crate::limit_resume::unwrapped_tail(lines, EXHAUSTED_SCAN_LINES)
                .iter()
                .find_map(|l| exhausted_limit_window(l))
        })
}

/// フッターの角括弧からモデル表示名を取り出す（#702）。
///
/// 実測の形: `  [Opus 5 (1M context) · xH]  user@example.com`。
/// **既知のモデルファミリで始まるときだけ**採る — TUI の表示は版で変わるので、
/// 知らない文字列をモデル名としてヘッダに出すより、何も出さないほうが良い
/// claude の**組み込み**コンテキスト表示から使用率を読む（Issue #1021）。
///
/// 2.1.258 のバイナリで確認した 3 形式（ユーザーが statusLine を設定していないときの表示）:
///
/// ```text
/// `${100 - pctLeft}% context used`        ← 使用率そのもの
/// `${pctLeft}% until auto-compact`        ← 自動圧縮までの残量（使用率 = 100 - N）
/// `Context low (${pctLeft}% remaining) …` ← 残量（使用率 = 100 - N）
/// ```
///
/// いずれも **% が語の前**に来るので、`ctx`/`context` の**後ろ**から探す従来の経路では
/// 取りこぼす（あるいは残量を使用率と誤読する）。
///
/// なお組み込み表示は `warn` 閾値（窓 − 33000 トークン）を超えるまで描かれないので、
/// これが読めるのは既に文脈が切迫しているときだけ（普段は transcript 側が答える）
fn builtin_ctx_percent(line: &str) -> Option<u32> {
    let lower = line.to_ascii_lowercase();
    if let Some(pct) = percent_before(&lower, "% context used") {
        return Some(pct.min(100));
    }
    if let Some(pct) = percent_before(&lower, "% until auto-compact") {
        return Some(100u32.saturating_sub(pct.min(100)));
    }
    if lower.contains("context low") {
        if let Some(pct) = percent_before(&lower, "% remaining") {
            return Some(100u32.saturating_sub(pct.min(100)));
        }
    }
    None
}

/// `…NN% <語>` の NN を読む（`suffix` は `%` から始まる語で、その**直前**の数字列を採る）
fn percent_before(lower: &str, suffix: &str) -> Option<u32> {
    let pos = lower.find(suffix)?;
    let digits: String = lower[..pos]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.chars().rev().collect::<String>().parse().ok()
}

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
///
/// alacritty_terminal の API がプラットフォームで分かれる: unix は `Pty::child()`
/// （`std::process::Child`）、Windows は `Pty::child_watcher().pid()`
/// （ConPTY 生成時の `GetProcessId`）。取得できなければ None
/// （対応付けが劣化するだけで、既存経路には影響しない）。
///
/// **`platform/` 配下ではなくここに置く**: 分岐しているのは OS ではなく
/// **外部クレートの型（`tty::Pty`）の API 形**なので、境界へ持ち上げると
/// `platform/` が alacritty へ依存してしまう（境界は自己完結を保つ）
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

/// tako-app が保持しているペインへの読み書きの手（#1200）。
///
/// [`TerminalSession::access`] で取り出す。**画面はメモリ上のグリッド**なので
/// 器へ問い合わせない（psmux はアウトオブプロセスの入力送出を持たないので、
/// ここが in-process ペインへの唯一の送り口になる）。
///
/// `TerminalSession` そのものは `&mut` を要る操作（`resize` / `feed_osc_bytes`）を
/// 持つのでスレッドを越えられない。こちらは**共有できるものだけ**を持つ
#[derive(Clone)]
pub struct PaneAccess {
    term: std::sync::Arc<FairMutex<Term<EventProxy>>>,
    notifier: Notifier,
}

impl std::fmt::Debug for PaneAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PaneAccess")
    }
}

impl PaneAccess {
    /// 可視画面の行（装飾なし）。[`TerminalSession::visible_lines`] と同じ材料
    pub fn visible_lines(&self) -> Vec<String> {
        screen::snapshot_opts(&self.term.lock(), &Theme::default(), true, 0.0)
            .lines
            .into_iter()
            .map(|l| l.text.trim_end().to_string())
            .collect()
    }

    /// PTY へバイト列を書く。**スクロール位置は触らない**
    /// （`TerminalSession::write` は最下部へ戻すが、ここは「いま画面に見えている
    /// ダイアログへ 1 キー送る」用途なので、読んだ画面と送る先をずらさない）
    pub fn write(&self, bytes: Vec<u8>) {
        use alacritty_terminal::event::Notify;
        self.notifier.notify(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// #946: ペインの端末申告は**呼び出し側の env に依らない**。
    /// 旧実装は既定を先に敷くだけだったので、`options.env` に TERM が 1 つ入れば
    /// 注入の意図が消え、消えたことはどのログにも出なかった
    #[test]
    fn ペインのtermはoptions_envで上書きされない() {
        for parent in [
            "tmux-256color",
            "screen-256color",
            "xterm-256color",
            "vt100",
            "",
        ] {
            let env = finalize_pane_env(
                map(&[
                    ("TERM", parent),
                    ("COLORTERM", "8bit"),
                    ("TAKO_PANE_ID", "1"),
                ]),
                false,
            );
            assert_eq!(
                env.get("TERM").map(String::as_str),
                Some("xterm-256color"),
                "options.env の TERM={parent:?} が残っている"
            );
            assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
            // 端末申告以外は素通し（この関数は TERM / COLORTERM 以外を触らない）
            assert_eq!(env.get("TAKO_PANE_ID").map(String::as_str), Some("1"));
        }
    }

    /// #946: 何も入っていない env にも必ず注入される（親からの継承に落ちない）
    #[test]
    fn ペインのtermは空のenvにも注入される() {
        let env = finalize_pane_env(std::collections::HashMap::new(), false);
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
    }

    /// #946 の注入口: `inherit` を立てると注入を外す = 親の値が子へ素通しになる
    #[test]
    fn 注入口inheritは端末申告を落とす() {
        let env = finalize_pane_env(
            map(&[("TERM", "tmux-256color"), ("TAKO_PANE_ID", "1")]),
            true,
        );
        assert_eq!(env.get("TERM"), None);
        assert_eq!(env.get("COLORTERM"), None);
        assert_eq!(env.get("TAKO_PANE_ID").map(String::as_str), Some("1"));
    }

    /// #946 の診断: 画面の TERMCHK 行を親の申告と突き合わせて理由を名指しする
    #[test]
    fn termchkの診断は継承と別の失敗を区別する() {
        let parent = Some("tmux-256color");
        // 注入どおり
        assert_eq!(
            diagnose_termchk(
                "TERMCHK=xterm-256color,truecolor",
                parent,
                Some("truecolor")
            ),
            TermCheck::Injected
        );
        // 親からの継承（TERM / COLORTERM とも一致）
        assert_eq!(
            diagnose_termchk("TERMCHK=tmux-256color,truecolor", parent, Some("truecolor")),
            TermCheck::Inherited {
                observed: "tmux-256color,truecolor".to_string()
            }
        );
        // TERM だけが親と一致（COLORTERM は注入が効いた形）も継承として扱う
        assert!(matches!(
            diagnose_termchk("TERMCHK=tmux-256color,", parent, None),
            TermCheck::Inherited { .. }
        ));
        // 継承では説明できない値
        assert!(matches!(
            diagnose_termchk("TERMCHK=dumb,", parent, Some("truecolor")),
            TermCheck::Unexpected { .. }
        ));
        // エコーが返っていない（打ったコマンド行だけが画面にある）
        assert_eq!(
            diagnose_termchk("$ echo \"TERMCHK=${TERM},${COLORTERM}\"", parent, None),
            TermCheck::NoEcho
        );
        assert_eq!(diagnose_termchk("", parent, None), TermCheck::NoEcho);
    }

    /// 画面末尾をそのまま渡しても、**最後に出た展開後の値**を拾う
    #[test]
    fn termchkの観測は展開後の最後の値を採る() {
        let screen = "$ echo \"TERMCHK=${TERM},${COLORTERM}\"\nTERMCHK=tmux-256color,truecolor\n$ echo \"TERMCHK=${TERM},${COLORTERM}\"\nTERMCHK=xterm-256color,truecolor\n$ ";
        assert_eq!(
            observed_termchk(screen).as_deref(),
            Some("xterm-256color,truecolor")
        );
        // セルフテストが渡すのは `pane_screen_tail` の ` | ` 連結 1 行（#946）。
        // 行区切りで切ると、次のペイン行まで値に混ざる
        let joined =
            "$ echo \"TERMCHK=${TERM},${COLORTERM}\" | TERMCHK=xterm-256color,truecolor | $ ";
        assert_eq!(
            observed_termchk(joined).as_deref(),
            Some("xterm-256color,truecolor")
        );
        // 値が末尾で終わる形（後ろに何も無い）
        assert_eq!(observed_termchk("TERMCHK=dumb,").as_deref(), Some("dumb,"));
    }

    /// 診断キーは grep できる形で固定する（ログの読み手が引く）
    #[test]
    fn termchkの診断キーは固定() {
        assert_eq!(TermCheck::Injected.label(), "injected");
        assert_eq!(
            TermCheck::Inherited {
                observed: "x".into()
            }
            .label(),
            "inherited"
        );
        assert_eq!(TermCheck::NoEcho.label(), "no-echo");
        assert_eq!(
            TermCheck::Unexpected {
                observed: "x".into()
            }
            .label(),
            "unexpected"
        );
    }

    /// 番犬（#946）: 端末申告の注入が **`options.env` の後**にあること。
    /// 既定として先に敷く形へ戻ると、呼び出し側の env 1 つで注入が無言で消える
    #[test]
    fn 端末申告の注入はoptions_envより後にある() {
        let src = include_str!("terminal.rs");
        let body = src
            .split("    pub fn spawn(")
            .nth(1)
            .expect("TerminalSession::spawn の定義");
        let body = &body[..body.find("\n    pub fn ").unwrap_or(body.len())];
        let options_env = body
            .find("env.extend(options.env)")
            .expect("spawn が options.env を取り込んでいない");
        let finalize = body
            .find("finalize_pane_env(")
            .expect("spawn が finalize_pane_env を通っていない（#946 の注入が消えている）");
        assert!(
            finalize > options_env,
            "端末申告の注入が options.env より前にある（呼び出し側の env で消える。#946）"
        );
        assert!(
            !body.contains("(\"TERM\".to_string()"),
            "spawn が TERM を直書きで敷いている（正本は PANE_TERMINAL_ENV。#946）"
        );
    }

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

    /// **#818**: スクロールバック上限は `SpawnOptions` で決まり、その行数で履歴が
    /// 打ち切られる。1,200 行流して上限 1,000 で止まることを実 PTY で固定する
    /// （直接ペインの飽和時フットプリントは `行 × 桁 × 24 B` に比例するので、
    /// ここが効かないと上限を下げても RAM は減らない）
    #[cfg(unix)]
    #[test]
    fn スクロールバック上限は設定した行数で履歴を打ち切る() {
        let session = spawn_lines(1_200, Some(1_000));
        assert_eq!(session.scrollback_limit(), 1_000);
        // 上限に達したら増えない = 到達を待てば値は安定する（時間には依らない）
        assert!(
            wait_history(&session, |h| h >= 1_000),
            "履歴が上限まで伸びない: {}",
            session.history_size()
        );
        assert_eq!(session.history_size(), 1_000, "上限を超えて保持している");
    }

    /// 対照: 既定（10,000 行）なら同じ 1,200 行がすべて残る。
    /// 上限が「効いている」ことを片側だけで主張しないための A 側
    #[cfg(unix)]
    #[test]
    fn 既定の上限では同じ出力がすべて履歴に残る() {
        let session = spawn_lines(1_200, None);
        assert_eq!(session.scrollback_limit(), crate::scrollback::DEFAULT_LINES);
        assert!(
            wait_history(&session, |h| h >= 1_100),
            "履歴が伸びない: {}",
            session.history_size()
        );
    }

    /// **#818**: 上限を下げると**既存ペインの履歴もその場で切り詰められる**
    /// （alacritty の `Grid::update_history` が `Row` を解放する）。
    /// これが無いと「設定を変えたのに再起動するまで RAM が戻らない」になる
    #[cfg(unix)]
    #[test]
    fn 上限を下げると既存ペインの履歴もその場で縮む() {
        let mut session = spawn_lines(1_200, None);
        assert!(
            wait_history(&session, |h| h >= 1_100),
            "履歴が伸びない: {}",
            session.history_size()
        );
        session.set_scrollback_limit(500);
        assert_eq!(session.scrollback_limit(), 500);
        assert_eq!(session.history_size(), 500, "既存の履歴が切り詰められない");
        // 範囲外は丸める（settings.json を手で書いた場合の経路）
        session.set_scrollback_limit(0);
        assert_eq!(session.scrollback_limit(), crate::scrollback::MIN_LINES);
    }

    /// 20 桁 4 行のペインへ `count` 行流す。上限は `limit`（None = 既定）
    #[cfg(unix)]
    fn spawn_lines(count: usize, limit: Option<usize>) -> TerminalSession {
        let script =
            format!("i=0; while [ $i -lt {count} ]; do printf 'L%d\\n' $i; i=$((i+1)); done");
        let (session, _rx) = TerminalSession::spawn(
            20,
            4,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), script],
                }),
                scrollback_lines: limit,
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");
        session
    }

    /// 履歴が条件を満たすまで待つ（満たしたら true。予算内に満たさなければ false）。
    ///
    /// 上限は [`crate::wait_budget::state_wait_budget`]（混み具合で**伸ばすだけ**・
    /// 4 倍で打ち切り）。固定窓だと混んだ機で「窓が短いのか / 出力が来ていないのか」が
    /// 出力から消える（#1308 と同じ理由）
    #[cfg(unix)]
    fn wait_history(session: &TerminalSession, done: impl Fn(usize) -> bool) -> bool {
        let busy = crate::wait_budget::machine_busy();
        let budget =
            crate::wait_budget::state_wait_budget(std::time::Duration::from_secs(30), busy);
        let start = std::time::Instant::now();
        while start.elapsed() < budget {
            if done(session.history_size()) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        done(session.history_size())
    }

    // --- #1301: 2 秒 tick が採る末尾 N 行 ---

    /// 実採取の画面（#1093 の 72 桁 / #1123 の 25 桁）で、全画面を渡したときと
    /// 末尾 `AGENT_TUI_TAIL_LINES` 行だけを渡したときのパース結果が一致すること。
    /// 窓を狭めた瞬間に「画面には出ているのに読めない」が起きないことの機械検査
    #[test]
    fn 末尾窓だけでも実採取画面のパース結果は変わらない() {
        let screens: Vec<Vec<String>> = vec![
            limit_stopped_screen("You've hit your session limit · resets 7:50pm (Asia/Tokyo)"),
            limit_stopped_screen("You've hit your weekly limit · resets 7:50pm (Asia/Tokyo)"),
            narrow_limit_stopped_screen(&[
                "  ⎿  You've hit your",
                "     session limit ·",
                "     resets 5:50am",
                "     (Asia/Tokyo)",
                "     /usage-credits to",
                "     request more usage",
                "     from your admin.",
            ]),
        ];
        for (i, full) in screens.iter().enumerate() {
            let tail: Vec<String> = full
                .iter()
                .rev()
                .take(AGENT_TUI_TAIL_LINES)
                .rev()
                .cloned()
                .collect();
            let a = parse_agent_metrics(full);
            let b = parse_agent_metrics(&tail);
            assert_eq!(
                (
                    a.as_ref().and_then(|m| m.limit_5h),
                    a.as_ref().and_then(|m| m.limit_week),
                    a.as_ref().and_then(|m| m.ctx_percent)
                ),
                (
                    b.as_ref().and_then(|m| m.limit_5h),
                    b.as_ref().and_then(|m| m.limit_week),
                    b.as_ref().and_then(|m| m.ctx_percent)
                ),
                "画面 {i}: 末尾 {AGENT_TUI_TAIL_LINES} 行だけだと結果が変わる（窓が足りない）"
            );
        }
    }

    /// 画面に `needle` が出るまで待つ（#1387。作法は #1308 と同じ）。
    ///
    /// 上限は [`crate::wait_budget::state_wait_budget`]（混み具合で**伸ばすだけ**・
    /// 4 倍で打ち切り）で、**尽きたらその場で panic** する = 呼び出し側が
    /// 「待ったつもりで検査せず進む」形にならない
    #[cfg(unix)]
    fn i1387_wait_visible(session: &TerminalSession, needle: &str, what: &str) {
        use std::time::{Duration, Instant};

        let busy = crate::wait_budget::machine_busy();
        let budget = crate::wait_budget::state_wait_budget(Duration::from_secs(20), busy);
        let start = Instant::now();
        let mut last: Vec<String> = Vec::new();
        while start.elapsed() < budget {
            last = session.visible_lines();
            if last.iter().any(|l| l.contains(needle)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "{what}: `{needle}` が出ない（waited={:.2?} budget={budget:?} busy={busy:?}）\n\
             画面: {last:?}",
            start.elapsed()
        );
    }

    /// #1387: 0 幅の結合文字（NFD の濁点 `U+3099` / アクセント `U+0301`）が
    /// 画面テキストの**4 経路すべて**に残ること。
    ///
    /// - `visible_lines()` … `screen::compose_line`（描画 / GUI モード / links の材料）
    /// - `tail_lines(n)` … `compose_grid_row`（#1301 / #651）
    /// - `history_plain_lines` … ペインログ（#112）
    /// - `selection_text()` … alacritty 自身の `selection_to_string`（Cmd+C）
    ///
    /// 4 つの一致を見るので**3 実装が同時に直っていること**を構造的に要求する
    /// （修正前は 4 番目だけが結合文字を残し、「コピーしたパスは開けるが画面の
    /// リンクは開けない」という非対称になっていた）
    #[cfg(unix)]
    #[test]
    fn 結合文字は画面テキストの全経路に残る() {
        const ROWS: usize = 6;
        // NFD: か + U+3099（全角 + 濁点）/ e + U+0301（半角 + アクセント）
        let nfd = "か\u{3099}_e\u{301}";
        let hist = format!("HIST_{nfd}");
        let vis = format!("VIS_{nfd}");
        // HIST は 8 行の詰め物で履歴へ押し出す（ROWS = 6）
        let script = format!(
            "printf '{hist}\\n'; \
             for i in 1 2 3 4 5 6 7 8; do printf 'PAD%s\\n' \"$i\"; done; \
             printf '{vis}\\n'; printf 'READY\\n'; sleep 30"
        );
        let (session, _rx) = TerminalSession::spawn(
            30,
            ROWS,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), script],
                }),
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");

        i1387_wait_visible(&session, "READY", "結合文字の 4 経路");

        // (1) 描画の材料
        let visible = session.visible_lines();
        assert!(
            visible.iter().any(|l| l == &vis),
            "描画の材料（visible_lines）から結合文字が落ちている: {visible:?}"
        );
        // (2) tail_lines は visible_lines の末尾と 1 バイトも変わらない
        assert_eq!(
            session.tail_lines(ROWS),
            visible,
            "tail_lines（compose_grid_row）が visible_lines と食い違う"
        );
        // (3) ペインログ（履歴へ押し出した行）
        let history = session.history_plain_lines(0, 20);
        assert!(
            history.iter().any(|l| l == &hist),
            "ペインログ（history_plain_lines）から結合文字が落ちている: {history:?}"
        );
        // (4) Cmd+C（alacritty の selection_to_string）と一致する
        let row = visible
            .iter()
            .position(|l| l == &vis)
            .expect("画面に VIS 行がある");
        session.start_selection(SelectionKind::Line, 0, row, false);
        let copied = session.selection_text().expect("選択テキストが取れる");
        assert_eq!(
            copied.trim_end(),
            vis,
            "画面テキストと選択コピー（selection_to_string）が食い違う\
             （#1387 の非対称）: {copied:?}"
        );
        session.clear_selection();
    }

    /// `tail_lines(n)` が `visible_lines()` の末尾 n 行と**1 バイトも変わらない**こと。
    /// 全角・末尾空白・空行・n が行数より大きい場合・スクロール中（display_offset > 0）を
    /// 実 PTY で通す（#1301 の入れ替えが「同じ文字列を返す」ことが前提になっている）
    #[cfg(unix)]
    #[test]
    fn tail_lines_は_visible_lines_の末尾と一致する() {
        use std::time::{Duration, Instant};

        // 20 桁 10 行。全角・末尾空白・行内空白・空行を混ぜる
        let script = "printf 'ab   \\nあいうえお\\na b\\n\\nTAIL_MARK\\n'; sleep 30";
        let (session, _rx) = TerminalSession::spawn(
            20,
            10,
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
        while !session.visible_lines().iter().any(|l| l == "TAIL_MARK") && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        let full = session.visible_lines();
        assert!(
            full.iter().any(|l| l == "TAIL_MARK"),
            "画面が整わない: {full:?}"
        );
        assert!(
            full.iter().any(|l| l == "あいうえお"),
            "全角行が出ていない: {full:?}"
        );

        for n in [0usize, 1, 2, 8, 10, 24, 48, 100] {
            let want: Vec<String> = full
                .iter()
                .rev()
                .take(n.min(full.len()))
                .rev()
                .cloned()
                .collect();
            assert_eq!(session.tail_lines(n), want, "n={n} で末尾が一致しない");
        }
        // alt screen（全画面 TUI）のペインでも同じ関係が成り立つ。
        // 副グリッドは履歴を持たないので `display_offset` は常に 0 = 起点の計算が変わる
        let alt_script = "printf '\\033[?1049h'; printf 'ALT_A\\nALT_B\\nALT_TAIL\\n'; sleep 30";
        let (alt, _rx3) = TerminalSession::spawn(
            20,
            6,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), alt_script.to_string()],
                }),
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !alt.is_alt_screen() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(alt.is_alt_screen(), "alt screen へ入らない");
        while !alt.visible_lines().iter().any(|l| l == "ALT_TAIL") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let alt_full = alt.visible_lines();
        assert!(
            alt_full.iter().any(|l| l == "ALT_TAIL"),
            "alt screen の画面が整わない: {alt_full:?}"
        );
        for n in [1usize, 3, 6, 48] {
            let want: Vec<String> = alt_full
                .iter()
                .rev()
                .take(n.min(alt_full.len()))
                .rev()
                .cloned()
                .collect();
            assert_eq!(alt.tail_lines(n), want, "alt screen の n={n} で一致しない");
        }

        // 空ペイン（1 行も書かれていない）でも同じ関係が成り立つ
        let (empty, _rx2) = TerminalSession::spawn(
            20,
            4,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), "sleep 30".to_string()],
                }),
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");
        assert_eq!(
            empty.tail_lines(48),
            empty.visible_lines(),
            "空ペインで末尾窓が全画面と一致しない"
        );
    }

    /// `visible_lines_filled()` が「折り返しの続きがありうる行」を**列**で見分けること（#651）。
    ///
    /// 実 PTY を狭い幅（10 桁）で張って、終了マーカー `__TAKO_EXIT=0`（13 文字）が
    /// 実際に割れる形を採る。ここで得た対が `tako_control::dispatch` 側の fixture
    /// （`exit_markerは折り返して割れていても拾える`）の実在の根拠になる
    #[cfg(unix)]
    #[test]
    fn visible_lines_filled_は折り返しを右端で見分ける() {
        use std::time::{Duration, Instant};

        // 10 桁。`__TAKO_EXIT=0` は 13 文字なので必ず 2 行に割れる
        let script = "printf '__TAKO_EXIT=0\\n'; sleep 30";
        let (session, _rx) = TerminalSession::spawn(
            10,
            6,
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
        while !session.visible_lines().iter().any(|l| l == "T=0") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let got = session.visible_lines_filled();
        // 本文は `visible_lines()` と 1 バイトも変わらない
        assert_eq!(
            got.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
            session.visible_lines(),
            "本文が visible_lines と一致しない: {got:?}"
        );
        let at = got
            .iter()
            .position(|(t, _)| t == "__TAKO_EXI")
            .unwrap_or_else(|| panic!("割れたマーカーが出ていない: {got:?}"));
        assert!(
            got[at].1,
            "右端まで埋まった行を「埋まっていない」と読んだ: {got:?}"
        );
        assert_eq!(got[at + 1].0, "T=0", "続きの行が違う: {got:?}");
        assert!(
            !got[at + 1].1,
            "10 桁に届かない行を「埋まっている」と読んだ: {got:?}"
        );

        // 全角で右端まで埋まった行（`あいうえお` = 5 文字 / 10 列）。**文字数で測ると
        // 「埋まっていない」と読む**ので、#325 の形（全角プロンプト + マーカー）が
        // 折り返したときに拾えなくなる
        let wide = "printf 'あいうえお__TAKO_EXIT=1\\n'; sleep 30";
        let (session2, _rx2) = TerminalSession::spawn(
            10,
            6,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), wide.to_string()],
                }),
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !session2.visible_lines().iter().any(|l| l == "T=1") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let got2 = session2.visible_lines_filled();
        let at = got2
            .iter()
            .position(|(t, _)| t == "あいうえお")
            .unwrap_or_else(|| panic!("全角行が出ていない: {got2:?}"));
        assert_eq!(got2[at].0.chars().count(), 5, "全角行の文字数");
        assert!(
            got2[at].1,
            "全角で埋まった行を文字数で測って「埋まっていない」と読んだ: {got2:?}"
        );
        assert_eq!(got2[at + 1].0, "__TAKO_EXI", "続きの行が違う: {got2:?}");
        assert!(got2[at + 1].1, "続きの行も右端まで埋まっている: {got2:?}");
        assert_eq!(got2[at + 2].0, "T=1", "続きの行が違う: {got2:?}");
    }

    /// **限界の固定**: `visible_lines_filled` は**行末が全角**の行を
    /// 「右端まで埋まっていない」と読む（#1389）。
    ///
    /// alacritty の `line_length()` は末尾から `cell.c != ' '` のセルを探すので、
    /// 全角の後続セル（`WIDE_CHAR_SPACER`）を**空きと数える**。
    /// [`TerminalSession::visible_lines_filled`] の doc に書いた「既知の限界」の
    /// 機械検査で、**判定を直すと落ちる** = 直すときは doc と
    /// `.agent/conventions.md` の #1283 節も同じコミットで直すこと。
    ///
    /// 10 桁の実 PTY で採った 6 形（2026-09-12 実測）:
    ///
    /// ```text
    /// A 右端が全角・以降の出力なし  abcdefghあ  filled=false line_length=9  WIDE_CHAR_SPACER
    /// B 右端が全角・続きあり        abcdefghあ  filled=true  line_length=10 WRAPLINE | WIDE_CHAR_SPACER
    /// C 右端が半角でちょうど埋まる  abcdefghij  filled=true  line_length=10
    /// D 途中に全角・右端は半角      あいうabcd  filled=true  line_length=10
    /// E CUP で描き直し・右端が全角  abcdefghあ  filled=false line_length=9  WIDE_CHAR_SPACER
    /// F 右端に届かない              abc         filled=false line_length=3
    /// ```
    ///
    /// 限界は **A / E**（物理的には 10 桁とも埋まっているのに偽）。B だけ真になるのは
    /// alacritty 自身が折り返して `WRAPLINE` を立てたからで、**器（tmux / psmux）が
    /// `CUP` で描き直した行（= E）は救えない**。C / D / F は従来どおりで、
    /// 全角が**途中**に在るだけなら列で正しく測れている（文字数で測る実装との差は
    /// `visible_lines_filled_は折り返しを右端で見分ける` が押さえる）
    #[cfg(unix)]
    #[test]
    fn visible_lines_filled_は行末の全角を取りこぼす() {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::{Flags, LineLength};

        /// 10 桁のペインへ 1 行だけ描く形
        struct Form {
            /// 形の名前（診断に出す）
            name: &'static str,
            /// 描く指令。**行末で出力を止める**（改行しない）
            script: &'static str,
            /// 描き終わりの目印（これが画面に出るまで待つ）
            needle: &'static str,
            /// 期待する画面テキスト（`visible_lines_filled()[0].0`）
            text: &'static str,
            /// 期待する `filled`（**A / E の false が限界**）
            filled: bool,
            /// 期待する `line_length()`
            line_length: usize,
            /// 右端セルが全角の後続セル（`WIDE_CHAR_SPACER`）か
            spacer_at_edge: bool,
        }

        /// 幅。全角 1 つ（2 列）+ 半角 8 つでちょうど埋まる
        const COLS: usize = 10;

        let forms = [
            Form {
                name: "A 右端が全角・以降の出力なし",
                script: "printf 'abcdefghあ'; sleep 30",
                needle: "abcdefghあ",
                text: "abcdefghあ",
                filled: false,
                line_length: 9,
                spacer_at_edge: true,
            },
            Form {
                name: "B 右端が全角・続きあり",
                script: "printf 'abcdefghあxyz'; sleep 30",
                needle: "xyz",
                text: "abcdefghあ",
                filled: true,
                line_length: 10,
                spacer_at_edge: true,
            },
            Form {
                name: "C 右端が半角でちょうど埋まる",
                script: "printf 'abcdefghij'; sleep 30",
                needle: "abcdefghij",
                text: "abcdefghij",
                filled: true,
                line_length: 10,
                spacer_at_edge: false,
            },
            Form {
                name: "D 途中に全角・右端は半角",
                script: "printf 'あいうabcd'; sleep 30",
                needle: "あいうabcd",
                text: "あいうabcd",
                filled: true,
                line_length: 10,
                spacer_at_edge: false,
            },
            Form {
                // 器（tmux / psmux）の再描画を `CUP` で模した形。埋まっていた行を
                // 描き直しても `WRAPLINE` は立たないので A と同じ穴に落ちる
                name: "E CUP で描き直し・右端が全角",
                script:
                    "printf '0123456789\\n'; printf '\\033[1;1H'; printf 'abcdefghあ'; sleep 30",
                needle: "abcdefghあ",
                text: "abcdefghあ",
                filled: false,
                line_length: 9,
                spacer_at_edge: true,
            },
            Form {
                name: "F 右端に届かない",
                script: "printf 'abc'; sleep 30",
                needle: "abc",
                text: "abc",
                filled: false,
                line_length: 3,
                spacer_at_edge: false,
            },
        ];

        for form in forms {
            let (session, _rx) = TerminalSession::spawn(
                COLS,
                6,
                SpawnOptions {
                    command: Some(SpawnCommand {
                        program: "/bin/sh".to_string(),
                        args: vec!["-c".to_string(), form.script.to_string()],
                    }),
                    ..SpawnOptions::default()
                },
            )
            .expect("PTY を張れる");
            // 待ちは状態待ち（`state_wait_budget` + 尽きたらその場で panic。#1308 の作法）
            i1387_wait_visible(&session, form.needle, form.name);
            let child = session.child_pid();
            let got = session.visible_lines_filled();
            let (line_length, spacer_at_edge, edge_flags) = {
                let term = session.term.lock();
                let grid = term.grid();
                assert_eq!(grid.columns(), COLS, "{}: 幅が {COLS} 桁でない", form.name);
                let row = &grid[Line(0)];
                let edge = &row[Column(COLS - 1)];
                (
                    row.line_length().0,
                    edge.flags.contains(Flags::WIDE_CHAR_SPACER),
                    format!("{:?}", edge.flags),
                )
            };
            // 自分が張った PTY の子だけを倒す（名前一致の `pkill` は使わない）
            drop(session);
            if let Some(pid) = child {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            }
            assert_eq!(
                got[0].0, form.text,
                "{}: 画面テキストが違う（形の前提が崩れている）: {got:?}",
                form.name
            );
            assert_eq!(
                line_length, form.line_length,
                "{}: line_length が違う（alacritty の仕様が変わった? edge_flags={edge_flags}）",
                form.name
            );
            assert_eq!(
                spacer_at_edge, form.spacer_at_edge,
                "{}: 右端の全角スペーサーの有無が違う（edge_flags={edge_flags}）",
                form.name
            );
            assert_eq!(
                got[0].1, form.filled,
                "{}: filled が違う（line_length={line_length} edge_flags={edge_flags}）。\n\
                 行末が全角の A / E で true になったなら**限界を直した**ので、\n\
                 `visible_lines_filled` の doc の「既知の限界」と\n\
                 `.agent/conventions.md` の #1283 節も同じコミットで直すこと: {got:?}",
                form.name
            );
        }
    }

    /// スクロール中（`display_offset > 0`）でも `tail_lines` と
    /// `visible_lines_filled` は**いま見えている**ビューポートを指すこと。
    ///
    /// 2 つはビューポートの起点を `display_offset` から**別々の式**で出す
    /// （`tail_lines` = `rows - d - take` / `visible_lines_filled` = `-d`）ので、
    /// 片方だけ拘束すると式を触ったときに気づけない（#1390）。
    /// 起点を取り違えると履歴の行が混ざる
    #[cfg(unix)]
    #[test]
    fn スクロール中でも末尾窓はビューポートを指す() {
        let session = spawn_lines(60, None);
        assert!(
            wait_history(&session, |h| h >= 40),
            "履歴が伸びない: {}",
            session.history_size()
        );
        session.scroll_to(20);
        assert_eq!(
            session.display_offset(),
            20,
            "前提: スクロール位置が届いている（届かないと最下部の検査になる）"
        );
        // `visible_lines_filled` の本文はビューポート（= `visible_lines`）と
        // 1 バイトも変わらない。**最下部でしか走らない**検査だと `-d` の項が死ぬ
        let filled = session.visible_lines_filled();
        assert_eq!(
            filled.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
            session.visible_lines(),
            "scroll_to(20) で visible_lines_filled の本文がビューポートと食い違う: {filled:?}"
        );
        assert_eq!(
            filled.len(),
            session.visible_lines().len(),
            "行数がビューポートと違う: {filled:?}"
        );
        for n in [1usize, 2, 4, 48] {
            let full = session.visible_lines();
            let want: Vec<String> = full
                .iter()
                .rev()
                .take(n.min(full.len()))
                .rev()
                .cloned()
                .collect();
            assert_eq!(
                session.tail_lines(n),
                want,
                "scroll_to(20) の n={n} で末尾が一致しない"
            );
        }
        session.scroll_to_bottom();
    }

    /// 届いているイベントを全部処理して、上がった通知を返す（#1390）。
    ///
    /// `title()` は `process_event` を通さないと更新されない（OSC は IO スレッドから
    /// チャネルで来る）ので、タイトルを読む検査はここを通す
    #[cfg(unix)]
    fn drain_notices(
        session: &mut TerminalSession,
        rx: &mut UnboundedReceiver<SessionEvent>,
    ) -> Vec<SessionNotice> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let Some(notice) = session.process_event(ev) {
                out.push(notice);
            }
        }
        out
    }

    /// **#1390**: ペインログ（`history_plain_lines`）と画面テキスト（`visible_lines`）が
    /// **同じ行**に対して 1 バイトも変わらないこと。
    ///
    /// 2 つは「セルから文字を取り出す規則」を共有する（#1387 で `screen` の 1 実装へ
    /// 寄せ、#1390 で末尾トリムのループも `compose_grid_row` 1 本へ寄せた）。
    /// 片方だけ直る形へ戻ると**ペインログだけ**黙って別の文字列になる。
    ///
    /// 検査は「履歴行をスクロールで可視化する」形にしてある: `display_offset = d`
    /// のビューポートは `Line(-d ..= rows-d-1)` なので、`d >= rows` なら
    /// **ビューポートの全行が履歴行**で、`history_plain_lines(d-rows, rows)` と
    /// 1:1 で対応する（`out[j]` = 履歴 offset `d-j`）= 同じ行を 2 経路で読める
    #[cfg(unix)]
    #[test]
    fn 履歴の平文行は可視行と1バイトも変わらない() {
        const COLS: usize = 20;
        const ROWS: usize = 6;

        // 全角 / 末尾空白 / 空行 / 行内の空白を混ぜ、そのあと詰め物で履歴へ送る
        let script = "printf 'H_ab   \\nH_あいうえ\\n\\nH_a b\\nH_TAIL\\n'; \
                      i=0; while [ $i -lt 10 ]; do printf 'PAD%d\\n' $i; i=$((i+1)); done; \
                      printf 'READY\\n'; sleep 30";
        let (session, _rx) = TerminalSession::spawn(
            COLS,
            ROWS,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), script.to_string()],
                }),
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");

        i1387_wait_visible(&session, "READY", "履歴と可視行の一致");
        let history = session.history_size();
        assert!(
            history >= ROWS + 4,
            "検査対象が履歴へ出ていない: history={history}"
        );

        // 履歴のすべての行が、どこかの窓に 1 度は入る（offset 1..=history を網羅）
        let mut seen: Vec<String> = Vec::new();
        for skip in 0..=(history - ROWS) {
            let d = skip + ROWS;
            session.scroll_to(d);
            assert_eq!(
                session.display_offset(),
                d,
                "スクロール位置が届かない（skip={skip}）"
            );
            let visible = session.visible_lines();
            let hist = session.history_plain_lines(skip, ROWS);
            assert_eq!(
                hist, visible,
                "skip={skip} でペインログと画面テキストが食い違う\n\
                 （履歴 {hist:?} / 可視 {visible:?}）"
            );
            seen.extend(visible);
        }
        session.scroll_to_bottom();

        // 比べた窓に「全角 / 末尾空白が落ちた行 / 行内の空白 / 空行」が実際に入っていた
        for needle in ["H_あいうえ", "H_ab", "H_a b", "H_TAIL"] {
            assert!(
                seen.iter().any(|l| l == needle),
                "検査対象 {needle:?} が比べた窓に入っていない: {seen:?}"
            );
        }
        assert!(
            seen.iter().any(|l| l.is_empty()),
            "空行が比べた窓に入っていない: {seen:?}"
        );
    }

    /// **#1390**: `set_scrollback_limit` が他の設定を巻き添えにしないこと。
    ///
    /// `Term::set_options` は `Config` を**丸ごと差し替える**ので、上限だけを渡すと
    /// `kitty_keyboard` が既定（false）へ戻り、`mode.remove(KITTY_KEYBOARD_PROTOCOL)`
    /// で Shift+Enter の区別（#28）が死ぬ。`term_config` の doc
    /// 「上限の動的変更で他の設定が巻き添えで既定へ戻ることはない」を守る検査。
    ///
    /// タイトルも見る: alacritty 0.26.0 の `set_options` は履歴を縮める前に
    /// **必ず `Event::Title` か `Event::ResetTitle` を送る**（実測: 上限を変えるたびに
    /// `TitleChanged` が 1 回上がる）。`Some(title)` なら同じ値の `Title` なので
    /// 値は変わらない = 余計な再描画 1 回で済む、という前提をここで固定する
    #[cfg(unix)]
    #[test]
    fn 上限の変更は他の設定を巻き添えにしない() {
        // OSC 2 でタイトルを設定し、CSI > 1 u で kitty keyboard の disambiguate を立てる
        let script = "printf '\\033]2;TAKO1390\\007'; printf '\\033[>1u'; \
                      printf 'READY\\n'; sleep 30";
        let (mut session, mut rx) = TerminalSession::spawn(
            20,
            6,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), script.to_string()],
                }),
                scrollback_lines: Some(2_000),
                ..SpawnOptions::default()
            },
        )
        .expect("PTY を張れる");

        i1387_wait_visible(&session, "READY", "上限変更の副作用");
        // タイトルは `process_event` を通してはじめて入る（READY より前に送られた
        // OSC なので、READY が見えている時点でイベントは届いている）
        let before_notices = drain_notices(&mut session, &mut rx);
        assert_eq!(
            session.title(),
            Some("TAKO1390"),
            "前提: OSC 2 のタイトルが入っている（通知 {before_notices:?}）"
        );
        assert!(
            session.disambiguate_keys(),
            "前提: CSI > 1 u が効いている（= `term_config` の `kitty_keyboard`）"
        );
        assert_eq!(session.scrollback_limit(), 2_000, "前提: 上限の初期値");

        // 上限を実際に変える（同値なら `set_options` を呼ばないので検査が空振りする）
        session.set_scrollback_limit(1_000);
        assert_eq!(session.scrollback_limit(), 1_000, "上限が変わっていない");

        assert!(
            session.disambiguate_keys(),
            "上限を変えたら kitty keyboard の disambiguate が落ちた\n\
             （`set_options` に `term_config` を渡していない = Shift+Enter の区別が死ぬ）"
        );
        assert_eq!(
            session.title(),
            Some("TAKO1390"),
            "上限を変えたらタイトルが変わった"
        );
        // `set_options` が送った通知を処理してもタイトルは変わらない
        // （`ResetTitle` が来ると None へ落ちる = 巻き添え）
        let after = drain_notices(&mut session, &mut rx);
        let titles = after
            .iter()
            .filter(|n| matches!(n, SessionNotice::TitleChanged))
            .count();
        assert_eq!(
            session.title(),
            Some("TAKO1390"),
            "上限変更の通知を処理したらタイトルが消えた（TitleChanged {titles} 件）"
        );
        assert!(
            session.disambiguate_keys(),
            "上限変更の通知を処理したら disambiguate が落ちた"
        );
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
    fn claudeの組み込みコンテキスト表示3形式を読む() {
        // #1021: 2.1.258 のバイナリから採った実文言。ユーザーが statusLine を
        // 設定していないときはこの 3 形式しか出ない
        // ① 使用率そのもの（% が "context" の**前**にあるので旧実装は取りこぼした）
        let m = parse_agent_metrics(&["30% context used".to_string()]).unwrap();
        assert_eq!(m.ctx_percent, Some(30));
        // ② 自動圧縮までの残量 → 使用率は 100 - N
        let m = parse_agent_metrics(&["15% until auto-compact".to_string()]).unwrap();
        assert_eq!(m.ctx_percent, Some(85));
        // ③ 残量表記（旧実装は 12% 使用と**誤読**していた。実際は 88% 使用）
        let m = parse_agent_metrics(&[
            "Context low (12% remaining) · Run /compact to compact & continue".to_string(),
        ])
        .unwrap();
        assert_eq!(m.ctx_percent, Some(88));
        // ③ の区切りつき（`Context low (12% remaining) · <warning>`）
        let m =
            parse_agent_metrics(
                &["Context low (3% remaining) · Context limit reached".to_string()],
            )
            .unwrap();
        assert_eq!(m.ctx_percent, Some(97));
    }

    #[test]
    fn 組み込み表示の追加で既存のctx表記が壊れていない() {
        // #1021 の追加が先に走るので、従来形式の回帰を明示的に固定する
        // ユーザー statusline（この機の実物）
        let m = parse_agent_metrics(&["  ctx  33% ███░░░░░░░".to_string()]).unwrap();
        assert_eq!(m.ctx_percent, Some(33));
        // codex の残量表記
        let m = parse_agent_metrics(&["Context 40% left".to_string()]).unwrap();
        assert_eq!(m.ctx_percent, Some(60));
        // `Context NN% used`（% が後ろに来る形）
        let m = parse_agent_metrics(&["Context 40% used".to_string()]).unwrap();
        assert_eq!(m.ctx_percent, Some(40));
        // 「remaining」を含むが `Context low` ではない行は組み込み判定に食われない
        let m = parse_agent_metrics(&["ctx 20% · 80% remaining".to_string()]).unwrap();
        assert_eq!(m.ctx_percent, Some(20));
    }

    #[test]
    fn 行の列から公開の入口でも同じ結果になる() {
        // #1021: dispatch は `agent_metrics_from_*` を通る。セッション経由と同値であること
        let lines = vec![
            "  ctx  41% ████░░░░░░".to_string(),
            "  5h   76%".to_string(),
        ];
        let via_lines = agent_metrics_from_lines(&lines).unwrap();
        let via_text = agent_metrics_from_text(&lines.join("\n")).unwrap();
        assert_eq!(via_lines.ctx_percent, Some(41));
        assert_eq!(via_text.ctx_percent, via_lines.ctx_percent);
        assert_eq!(via_text.limit_5h, via_lines.limit_5h);
        assert!(agent_metrics_from_text("").is_none());
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

    /// #1093: 上限で止まったペインの実画面（見出し + 入力欄 + `5h` の無いフッター）。
    /// 幾何は稼働中の実ペインから採った形（フッター 6 行 / 入力欄 3 行 / あいだの空行）
    fn limit_stopped_screen(headline: &str) -> Vec<String> {
        let mut lines: Vec<String> = vec!["⏺ 実装を進めます".into(), String::new()];
        lines.push(format!("  ⎿  {headline}"));
        lines.push("     /usage-credits to request more usage from your admin.".into());
        lines.push(String::new());
        lines.push("─".repeat(72));
        lines.push("❯".into());
        lines.push("─".repeat(72));
        // フッター 6 行。**`5h NN%` / `7d NN%` は出ていない**（= 症状どおり）
        for l in [
            "  ⏵⏵ accept edits on",
            "  tako",
            "  main",
            "  ~/dev/tako",
            "  ⏸ 待機中 · ? for shortcuts",
            "",
        ] {
            lines.push(l.into());
        }
        lines
    }

    #[test]
    fn issue1093_上限で止まった画面はメーターを100パーセントで埋める() {
        // 旧実装はフッターの `5h NN%` だけを見ていたので、上限で止まった瞬間に
        // メトリクスごと None（ステータスバーは `--`）になっていた
        let m = parse_agent_metrics(&limit_stopped_screen(
            "You've hit your session limit · resets 7:50pm (Asia/Tokyo)",
        ))
        .expect("上限中はメトリクスが取れる（`--` にしない）");
        assert_eq!(
            m.limit_5h,
            Some(100),
            "session limit = 5h 枠を使い切っている"
        );
        assert_eq!(m.limit_week, None, "週枠については何も言っていない");
        assert_eq!(m.source, MetricsSource::Claude);

        // 週枠なら 7d 側が埋まる
        let m = parse_agent_metrics(&limit_stopped_screen(
            "You've hit your weekly limit · resets 7:50pm (Asia/Tokyo)",
        ))
        .expect("取れる");
        assert_eq!(m.limit_5h, None);
        assert_eq!(m.limit_week, Some(100));
    }

    #[test]
    fn issue1093_フッターの実データは上限の見出しに上書きされない() {
        // 復帰してフッターが数値を出し始めたら、そちらが正（100% に貼り付かない）
        let mut lines =
            limit_stopped_screen("You've hit your session limit · resets 7:50pm (Asia/Tokyo)");
        lines.push("  ctx 21%  5h 12%  7d 46%".into());
        let m = parse_agent_metrics(&lines).expect("取れる");
        assert_eq!(
            m.limit_5h,
            Some(12),
            "実データを 100% で塗り潰してはいけない"
        );
        assert_eq!(m.limit_week, Some(46));
    }

    /// #1123: **幅 25 桁**で上限に当たったペインの実画面（2026-09-04 の実採取）。
    /// 見出しは claude 自身が語の境界で折り返し、続きを内容の列へ字下げする。
    /// フッターの `5h` / `7d` が `--` なのも実際の症状どおり
    fn narrow_limit_stopped_screen(headline: &[&str]) -> Vec<String> {
        let mut lines: Vec<String> = vec!["⏺ 実装を進めます".into(), String::new()];
        for l in headline {
            lines.push((*l).into());
        }
        lines.push(String::new());
        lines.push("─".repeat(25));
        lines.push("❯".into());
        lines.push("─".repeat(25));
        for l in [
            "  [Opus 5 · xH]  ▸ z…",
            "  ctx   7% ░░░░░░░░░░",
            "  5h   --",
            "  7d   --",
            "  ⏵⏵ auto mode on    ·",
        ] {
            lines.push(l.into());
        }
        lines
    }

    #[test]
    fn issue1123_折り返した見出しでもメーターを100パーセントで埋める() {
        // 実採取（25 桁）。#1093 の規則は 1 行を入力に取るので、狭いペインでは
        // どの物理行にも当たらず `5h` が `--` のままだった
        let m = parse_agent_metrics(&narrow_limit_stopped_screen(&[
            "  ⎿  You've hit your",
            "     session limit ·",
            "     resets 5:50am",
            "     (Asia/Tokyo)",
            "     /usage-credits to",
            "     request more usage",
            "     from your admin.",
        ]))
        .expect("上限中はメトリクスが取れる（`--` にしない）");
        assert_eq!(m.limit_5h, Some(100), "折り返した `session limit` = 5h 枠");
        assert_eq!(m.limit_week, None, "週枠については何も言っていない");

        // 週枠も同じ（折り返し位置が変わるだけ）
        let m = parse_agent_metrics(&narrow_limit_stopped_screen(&[
            "  ⎿  You've hit your",
            "     weekly limit ·",
            "     resets 5:50am",
            "     (Asia/Tokyo)",
        ]))
        .expect("取れる");
        assert_eq!(m.limit_5h, None);
        assert_eq!(m.limit_week, Some(100));
    }

    #[test]
    fn issue1123_折り返しても枠の分からない上限ではメーターを埋めない() {
        // 結合しても `usage credit limit` は 5h / 週へ対応づけられない
        // （この画面は `ctx 7%` を出しているのでメトリクス自体は取れる。
        //   見るのは**枠のメーターを埋めていないこと**）
        let m = parse_agent_metrics(&narrow_limit_stopped_screen(&[
            "  ⎿  You've hit your",
            "     usage credit limit",
            "     · contact your",
            "     admin to increase",
            "     it",
        ]))
        .expect("ctx があるので取れる");
        assert_eq!(m.limit_5h, None, "枠が分からないのにメーターを埋めている");
        assert_eq!(m.limit_week, None, "枠が分からないのにメーターを埋めている");
    }

    #[test]
    fn issue1093_枠の分からない上限ではメーターを埋めない() {
        // `usage credit limit` は 5h / 週へ対応づけられない = メーターに嘘を書かない
        assert!(
            parse_agent_metrics(&limit_stopped_screen(
                "You've hit your usage credit limit · contact your admin to increase it",
            ))
            .is_none(),
            "枠が分からないのにメーターを埋めている"
        );
        // 上限ではない通常の idle でも埋めない
        assert!(parse_agent_metrics(&limit_stopped_screen(
            "実装が完了しました。テストは全て緑です。"
        ))
        .is_none());
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
}

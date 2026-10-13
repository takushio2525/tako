//! mouse_forward — マウス報告を要求する TUI へ左クリックを渡す（Issue #1961）
//!
//! 全画面の Claude Code は直接ペインでも器（tmux）の中でもマウス報告を要求する
//! （`alternate_on=1` / `mouse_any_flag=1` を実測）。mod のボタン・Markdown のリンク・
//! 「Jump to bottom (click)」はクリックで押す前提なのに、#1961 前の tako は
//! **ホイールしか**内側アプリへ渡しておらず（`terminal::wheel_report_bytes`）、
//! 左クリックは常に tako 自身の選択になっていた。
//!
//! ここは「渡すか / tako が取るか」の判断（[`route`]）と、渡すときのバイト列
//! （[`click_report_bytes`]）の**1 実装**。GUI のクリック（tako-app）と AI のクリック
//! （CLI `tako click` / MCP `tako_send_input` の `click`）が同じ判断と同じバイト列を通る。
//!
//! # 判断（Issue #1961 のコメント = ユーザーから一任された master の決定）
//!
//! - **既定 on**。マウス報告を要求する TUI には素の左クリック / ドラッグを渡す
//!   （iTerm2 / Terminal.app と同じ慣習）。設定で off に戻せ、切り替えは即時に効く
//! - **Option（⌥）+ ドラッグで tako 側の選択**。Shift も同じ扱いにする
//!   （alacritty / kitty / WezTerm / Windows Terminal の慣習。Windows に ⌥ は無い）
//! - **cmd+クリック（Windows は Ctrl）のリンクは今どおり tako が取る**
//! - ペインのフォーカスを移すクリック・ウィンドウを前面へ出すクリックは渡さない
//!   （作業ペインを選ぶつもりのクリックで、TUI のボタンを誤って押さない）
//!
//! # 届け方
//!
//! - 直接ペイン: 端末モード（`TermMode`）からその場で分かるので PTY へ書く
//! - 器のペイン: 外側の端末モードは器の `set -g mouse on` で**常に**要求ありに見える
//!   （#1252 で実測）ので使えない。器のペイン側のフラグ（[`tmux_mouse_flags`]）を
//!   問い合わせ、`send-keys -H` で器へ直接注入する（[`send_tmux`]。#167 と同じ =
//!   外側 PTY 経由の断片化が構造的に起きない）
//! - ドラッグの移動は #167 と同じトークンバケット（[`MotionBudget`]）を通す。
//!   押す・離すは捨てない（離すを捨てると内側アプリが押しっぱなしだと思い込む）

use std::sync::OnceLock;

use alacritty_terminal::term::TermMode;

use crate::scroll::ScrollTarget;
use crate::tmux::run_tmux;

/// 既定は on（Issue #1961 の判断）
pub const DEFAULT_ENABLED: bool = true;

/// tako 側の選択へ切り替える修飾キーの表示名（応答・docs 用。判定は [`bypass_modifier`]）
pub const BYPASS_MODIFIERS: [&str; 2] = ["option", "shift"];

/// A/B（`TAKO_1961_LEGACY=1`）: #1961 前（左クリックは常に tako の選択）へ戻す。
/// **ゲートはこの 1 か所**で、tako-control / tako-app もここを読む
pub fn legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1961_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 内側アプリが要求しているマウス報告の符号化
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// SGR（`ESC[<b;x;yM` / `m`。1006）。座標の上限なし
    Sgr,
    /// UTF-8（1005）。座標を UTF-8 の 1〜2 バイトで表す（上限 2015）
    Utf8,
    /// 素の X10 / normal（各値 +32 の 1 バイト。上限 223）
    X10,
}

/// 内側アプリが要求しているクリックの報告の形
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseReport {
    /// ボタンを押したままの移動（ドラッグ）も報告するか（1002 / 1003）。
    /// 1000 だけのアプリへは押す・離すだけを渡す
    pub drag: bool,
    pub encoding: Encoding,
}

impl MouseReport {
    /// 端末モードから読む（直接ペイン）。報告を要求していなければ `None`
    pub fn from_term_mode(mode: TermMode) -> Option<Self> {
        if !mode.intersects(TermMode::MOUSE_MODE) {
            return None;
        }
        let encoding = if mode.contains(TermMode::SGR_MOUSE) {
            Encoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            Encoding::Utf8
        } else {
            Encoding::X10
        };
        Some(Self {
            drag: mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
            encoding,
        })
    }
}

/// 器のペイン側のマウス要求（`display-message` の 1 回ぶん）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TmuxMouseFlags {
    /// `#{mouse_any_flag}`（1000 / 1002 / 1003 のどれか）
    pub any: bool,
    /// `#{mouse_sgr_flag}`
    pub sgr: bool,
    /// `#{mouse_utf8_flag}`
    pub utf8: bool,
    /// `#{mouse_button_flag}`（1002）
    pub button: bool,
    /// `#{mouse_all_flag}`（1003）
    pub all: bool,
    /// `#{pane_in_mode}`（copy mode 中。注入したバイトが copy-mode のキーとして解釈される）
    pub in_mode: bool,
}

/// [`tmux_mouse_flags`] のフォーマット。**区切りはカンマ**（空白区切りだと器が知らない
/// フォーマットの空欄が畳まれて隣へずれる = #1252 の `probe_pane_mouse` と同じ理由）
pub const TMUX_FLAGS_FORMAT: &str = "#{mouse_any_flag},#{mouse_sgr_flag},#{mouse_utf8_flag},\
     #{mouse_button_flag},#{mouse_all_flag},#{pane_in_mode}";

impl TmuxMouseFlags {
    /// [`TMUX_FLAGS_FORMAT`] の応答 1 行を読む
    pub fn parse(line: &str) -> Self {
        let mut f = line.trim().split(',').map(|v| v.trim() == "1");
        let mut next = || f.next().unwrap_or(false);
        Self {
            any: next(),
            sgr: next(),
            utf8: next(),
            button: next(),
            all: next(),
            in_mode: next(),
        }
    }

    /// 報告の形。要求していなければ `None`
    pub fn report(&self) -> Option<MouseReport> {
        if !self.any {
            return None;
        }
        let encoding = if self.sgr {
            Encoding::Sgr
        } else if self.utf8 {
            Encoding::Utf8
        } else {
            Encoding::X10
        };
        Some(MouseReport {
            drag: self.button || self.all,
            encoding,
        })
    }
}

/// クリックの段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickPhase {
    /// 押す
    Press,
    /// 押したままの移動
    Drag,
    /// 離す
    Release,
}

impl ClickPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            ClickPhase::Press => "press",
            ClickPhase::Drag => "drag",
            ClickPhase::Release => "release",
        }
    }
}

/// 左ボタンの 1 イベントぶんのバイト列。`col` / `row` は 0 始まりの表示セル座標。
///
/// 渡さないもの（`None`）: ドラッグを要求していないアプリへのドラッグ・符号化の上限を
/// 超える座標（X10 は 223、UTF-8 は 2015。**丸めない** = 端のセルを押したことにすると
/// 押していない位置のボタンが押される。alacritty も同じく送らない）
pub fn click_report_bytes(
    report: MouseReport,
    phase: ClickPhase,
    col: usize,
    row: usize,
    ctrl: bool,
) -> Option<Vec<u8>> {
    if phase == ClickPhase::Drag && !report.drag {
        return None;
    }
    // 修飾は Ctrl だけ載せる（Shift / Option は tako 側の選択へ切り替える修飾なので
    // ここへは来ない = `bypass_modifier`）
    let mods: u32 = if ctrl { 16 } else { 0 };
    let (x, y) = (col + 1, row + 1);
    match report.encoding {
        Encoding::Sgr => {
            let (button, end) = match phase {
                ClickPhase::Press => (0, 'M'),
                ClickPhase::Drag => (32, 'M'),
                ClickPhase::Release => (0, 'm'),
            };
            Some(format!("\x1b[<{};{x};{y}{end}", button + mods).into_bytes())
        }
        Encoding::X10 | Encoding::Utf8 => {
            // 素の形式の離すはボタン番号を持たない（3 = どれかが離れた）
            let button: u32 = match phase {
                ClickPhase::Press => 0,
                ClickPhase::Drag => 32,
                ClickPhase::Release => 3,
            } + mods;
            let mut out = vec![0x1b, b'[', b'M', (32 + button) as u8];
            for v in [x, y] {
                encode_coord(&mut out, report.encoding, v)?;
            }
            Some(out)
        }
    }
}

/// 素の形式の座標 1 つ（1 始まり）を足す。表せなければ `None`
fn encode_coord(out: &mut Vec<u8>, encoding: Encoding, v: usize) -> Option<()> {
    let code = 32 + v;
    match encoding {
        Encoding::X10 if code <= 255 => out.push(code as u8),
        // UTF-8 は 2 バイトまで（1005 の仕様。上限 2047 = 座標 2015）
        Encoding::Utf8 if code < 0x80 => out.push(code as u8),
        Encoding::Utf8 if code <= 0x7ff => {
            out.push(0xc0 | (code >> 6) as u8);
            out.push(0x80 | (code & 0x3f) as u8);
        }
        _ => return None,
    }
    Some(())
}

/// tako 側の選択へ切り替える修飾か（Option / Shift。Issue #1961 の判断 + 慣習）
pub fn bypass_modifier(alt: bool, shift: bool) -> bool {
    alt || shift
}

/// クリックの置かれた状況（[`route`] の入力）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClickContext {
    /// 設定（`settings.json` の `mouse_click_forward`）
    pub enabled: bool,
    /// A/B（`TAKO_1961_LEGACY=1`）
    pub legacy: bool,
    /// リンクの修飾（macOS = cmd / Windows = Ctrl。#763 の 1 実装で判定済み）
    pub link_modifier: bool,
    /// tako 側の選択へ切り替える修飾（[`bypass_modifier`]）
    pub bypass_modifier: bool,
    /// ウィンドウを前面へ出すクリック（GPUI の `first_mouse`）
    pub window_activation: bool,
    /// このクリックでペインのフォーカスが移る
    pub focus_change: bool,
    /// 履歴を遡って見ている（器のミラー表示 / 直接ペインの `display_offset > 0`）。
    /// 見えている行と内側アプリの画面が一致しないので、座標を渡せない
    pub scrolled_back: bool,
}

/// tako が取る理由（応答と診断に出す語。`as_str` が正）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakoReason {
    Legacy,
    Disabled,
    LinkModifier,
    BypassModifier,
    WindowActivation,
    FocusChange,
    ScrolledBack,
    /// 内側アプリがマウス報告を要求していない（シェル・less 等）= 今までどおり
    NotRequested,
    /// 器のペインが copy mode 中（注入すると copy-mode のキーとして解釈される）
    CopyMode,
    /// 器へ問い合わせられなかった（器が消えた・上限で打ち切り）
    Unresolved,
    /// 内側アプリの報告形式で表せない座標（X10 の 223 桁超え）
    OutOfRange,
}

impl TakoReason {
    pub fn as_str(self) -> &'static str {
        match self {
            TakoReason::Legacy => "legacy",
            TakoReason::Disabled => "disabled",
            TakoReason::LinkModifier => "link_modifier",
            TakoReason::BypassModifier => "bypass_modifier",
            TakoReason::WindowActivation => "window_activation",
            TakoReason::FocusChange => "focus_change",
            TakoReason::ScrolledBack => "scrolled_back",
            TakoReason::NotRequested => "not_requested",
            TakoReason::CopyMode => "copy_mode",
            TakoReason::Unresolved => "unresolved",
            TakoReason::OutOfRange => "out_of_range",
        }
    }
}

/// 押したときの行き先
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickRoute {
    /// tako が扱う（選択・リンク = #1961 前と同じ）
    Tako(TakoReason),
    /// 内側アプリのマウス要求を確かめ、要求していれば渡す（[`decide`]）
    Forward,
}

/// 押したときの行き先を決める（内側アプリの要求を見る前の段）。
/// 優先順は「A/B → 設定 → 修飾 → フォーカス → 遡り」
pub fn route(ctx: &ClickContext) -> ClickRoute {
    let reason = if ctx.legacy {
        TakoReason::Legacy
    } else if !ctx.enabled {
        TakoReason::Disabled
    } else if ctx.link_modifier {
        TakoReason::LinkModifier
    } else if ctx.bypass_modifier {
        TakoReason::BypassModifier
    } else if ctx.window_activation {
        TakoReason::WindowActivation
    } else if ctx.focus_change {
        TakoReason::FocusChange
    } else if ctx.scrolled_back {
        TakoReason::ScrolledBack
    } else {
        return ClickRoute::Forward;
    };
    ClickRoute::Tako(reason)
}

/// 内側アプリの要求を見たあとの判断。`Ok` = この形で渡す
pub fn decide(report: Option<MouseReport>, in_copy_mode: bool) -> Result<MouseReport, TakoReason> {
    if in_copy_mode {
        return Err(TakoReason::CopyMode);
    }
    report.ok_or(TakoReason::NotRequested)
}

/// ドラッグの移動のレート制限（#167 と同じトークンバケット・同じ値）。
/// 移動は絶対座標なので、捨てても次の移動か離すで追いつく
#[derive(Debug, Clone, Copy)]
pub struct MotionBudget {
    tokens: f32,
    last: std::time::Instant,
}

impl Default for MotionBudget {
    fn default() -> Self {
        Self::new(std::time::Instant::now())
    }
}

impl MotionBudget {
    pub fn new(now: std::time::Instant) -> Self {
        Self {
            tokens: crate::terminal::WHEEL_FORWARD_BURST,
            last: now,
        }
    }

    /// 1 イベントぶんを取る。取れなければ false（その移動は捨てる）
    pub fn take(&mut self, now: std::time::Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f32();
        self.last = now;
        let (allowed, rest) = crate::terminal::wheel_rate_take(self.tokens, elapsed, 1);
        self.tokens = rest;
        allowed == 1
    }
}

/// 器のペインの在処。バックエンドはネストした tmux を辿る解決（子プロセス）が要るので、
/// **解決は background で**（[`Self::resolve`]）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxClickSource {
    /// 解決済み（取り込みビュー = ビュー先 / ミラーで解決した実体）
    Resolved(ScrollTarget),
    /// tako のバックエンドセッション（ペイン内で attach したネスト tmux を辿る）
    Backend { socket: String, session: String },
}

impl TmuxClickSource {
    /// ペインのクリックの送り先が器か（GUI と dispatch の 1 実装）。**取り込みビューを
    /// バックエンドより優先する**（#181 と同じ理由: persist ON ではビューペインの外側も
    /// バックエンドで包まれるが、表示の実体はビュー先にある）。どちらでもなければ直接ペイン
    pub fn for_pane(
        view: Option<&crate::tmux::TmuxView>,
        backend_session: Option<&str>,
        backend_socket: &str,
    ) -> Option<Self> {
        if let Some(view) = view {
            return Some(TmuxClickSource::Resolved(ScrollTarget::Nested {
                socket: view.socket.clone(),
                session: view.wrapper.clone().unwrap_or_else(|| view.session.clone()),
            }));
        }
        backend_session.map(|session| TmuxClickSource::Backend {
            socket: backend_socket.to_string(),
            session: session.to_string(),
        })
    }

    /// 送り先へ解決する（バックエンドは `scroll::resolve_target` = 子プロセス。background で呼ぶ）
    pub fn resolve(self) -> ScrollTarget {
        match self {
            TmuxClickSource::Resolved(t) => t,
            TmuxClickSource::Backend { socket, session } => {
                crate::scroll::resolve_target(&socket, &session, &[None, Some(&socket)])
            }
        }
    }
}

/// 器のペイン側のマウス要求を 1 回問い合わせる（上限つきの `run_tmux`。background で呼ぶ）。
/// 器を呼べなければ `None`
pub fn tmux_mouse_flags(target: &ScrollTarget) -> Option<TmuxMouseFlags> {
    let (socket, t) = target.locate();
    let out = run_tmux(
        socket,
        &["display-message", "-p", "-t", &t, TMUX_FLAGS_FORMAT],
    )
    .ok()?;
    Some(TmuxMouseFlags::parse(out.lines().next()?))
}

/// 器のペインへ報告のバイト列を直接注入する（`send-keys -H`。#167 と同じ経路。
/// background で呼ぶ）。失敗は返すだけ（呼び手が診断へ残す）
pub fn send_tmux(target: &ScrollTarget, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    let (socket, t) = target.locate();
    let mut args: Vec<String> = vec!["send-keys".into(), "-t".into(), t, "-H".into()];
    args.extend(bytes.iter().map(|b| format!("{b:02x}")));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_tmux(socket, &arg_refs).map(|_| ())
}

/// AI のクリック（押す + 離す）を器のペインへ渡す（CLI `tako click` / MCP `tako_send_input` の
/// `click` の器の経路。background で呼ぶ）。GUI のクリックと同じ判断（[`decide`]）と
/// 同じバイト列（[`click_report_bytes`]）を通る
pub fn click_tmux(target: &ScrollTarget, col: usize, row: usize) -> Result<MouseReport, String> {
    let flags = tmux_mouse_flags(target).ok_or_else(|| {
        format!(
            "器へ問い合わせられない（{}）",
            TakoReason::Unresolved.as_str()
        )
    })?;
    let report = decide(flags.report(), flags.in_mode).map_err(not_forwarded_message)?;
    let bytes = click_pair(report, col, row)?;
    send_tmux(target, &bytes)?;
    Ok(report)
}

/// 押す + 離すのバイト列（AI のクリック）
pub fn click_pair(report: MouseReport, col: usize, row: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for phase in [ClickPhase::Press, ClickPhase::Release] {
        bytes.extend(
            click_report_bytes(report, phase, col, row, false).ok_or_else(|| {
                format!(
                    "座標 (row {row}, col {col}) は内側アプリの報告形式（{:?}）で表せない",
                    report.encoding
                )
            })?,
        );
    }
    Ok(bytes)
}

/// 渡さなかった理由のメッセージ（AI のクリックのエラー本文。理由の語を必ず含める）
pub fn not_forwarded_message(reason: TakoReason) -> String {
    match reason {
        TakoReason::NotRequested => format!(
            "このペインのアプリはマウス報告を要求していない（{}）。\
             シェル・less 等へのクリックは意味を持たないので渡さない",
            reason.as_str()
        ),
        TakoReason::CopyMode => format!(
            "器のペインが copy mode 中（{}）。渡すと copy mode のキーとして解釈される",
            reason.as_str()
        ),
        other => format!("クリックを渡せない（{}）", other.as_str()),
    }
}

/// GUI のクリックの転送の累計（`tako mouse` / MCP `tako_scrollback` の応答。外から確かめる口）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForwardStats {
    pub press: u64,
    pub drag: u64,
    pub release: u64,
    /// 最後に押したクリックの行き先（ペイン, 渡したか, 理由の語）
    pub last: Option<(u64, bool, &'static str)>,
}

impl ForwardStats {
    pub fn count(&mut self, phase: ClickPhase) {
        match phase {
            ClickPhase::Press => self.press += 1,
            ClickPhase::Drag => self.drag += 1,
            ClickPhase::Release => self.release += 1,
        }
    }
}

/// クリックの転送の現在状態（CLI / MCP の応答の材料）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseForwardStatus {
    pub enabled: bool,
    pub legacy: bool,
    pub stats: ForwardStats,
}

impl Default for MouseForwardStatus {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
            legacy: legacy(),
            stats: ForwardStats::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SGR_DRAG: MouseReport = MouseReport {
        drag: true,
        encoding: Encoding::Sgr,
    };

    #[test]
    fn sgrは押す_ドラッグ_離すを1始まりの座標で組む() {
        assert_eq!(
            click_report_bytes(SGR_DRAG, ClickPhase::Press, 4, 9, false).unwrap(),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            click_report_bytes(SGR_DRAG, ClickPhase::Drag, 5, 9, false).unwrap(),
            b"\x1b[<32;6;10M"
        );
        assert_eq!(
            click_report_bytes(SGR_DRAG, ClickPhase::Release, 5, 9, false).unwrap(),
            b"\x1b[<0;6;10m"
        );
        // Ctrl は +16
        assert_eq!(
            click_report_bytes(SGR_DRAG, ClickPhase::Press, 0, 0, true).unwrap(),
            b"\x1b[<16;1;1M"
        );
    }

    #[test]
    fn x10は離すをボタン3で組み_上限を超える座標は送らない() {
        let x10 = MouseReport {
            drag: true,
            encoding: Encoding::X10,
        };
        assert_eq!(
            click_report_bytes(x10, ClickPhase::Press, 4, 9, false).unwrap(),
            vec![0x1b, b'[', b'M', 32, 32 + 5, 32 + 10]
        );
        assert_eq!(
            click_report_bytes(x10, ClickPhase::Release, 4, 9, false).unwrap(),
            vec![0x1b, b'[', b'M', 35, 32 + 5, 32 + 10]
        );
        assert_eq!(
            click_report_bytes(x10, ClickPhase::Drag, 4, 9, false).unwrap(),
            vec![0x1b, b'[', b'M', 64, 32 + 5, 32 + 10]
        );
        // 223 桁目（0 始まり 222）までは表せる・それを超えると丸めずに送らない
        assert!(click_report_bytes(x10, ClickPhase::Press, 222, 0, false).is_some());
        assert_eq!(
            click_report_bytes(x10, ClickPhase::Press, 223, 0, false),
            None
        );
    }

    #[test]
    fn utf8は128以上の座標を2バイトで表す() {
        let utf8 = MouseReport {
            drag: false,
            encoding: Encoding::Utf8,
        };
        // 座標 200（1 始まり 201）→ 233 = U+00E9 → C3 A9
        assert_eq!(
            click_report_bytes(utf8, ClickPhase::Press, 200, 0, false).unwrap(),
            vec![0x1b, b'[', b'M', 32, 0xc3, 0xa9, 33]
        );
        assert!(click_report_bytes(utf8, ClickPhase::Press, 2014, 0, false).is_some());
        assert_eq!(
            click_report_bytes(utf8, ClickPhase::Press, 2015, 0, false),
            None
        );
    }

    #[test]
    fn ドラッグを要求しないアプリへはドラッグを渡さない() {
        let click_only = MouseReport {
            drag: false,
            encoding: Encoding::Sgr,
        };
        assert_eq!(
            click_report_bytes(click_only, ClickPhase::Drag, 1, 1, false),
            None
        );
        assert!(click_report_bytes(click_only, ClickPhase::Release, 1, 1, false).is_some());
    }

    #[test]
    fn 端末モードから報告の形を読む() {
        assert_eq!(MouseReport::from_term_mode(TermMode::ALT_SCREEN), None);
        assert_eq!(
            MouseReport::from_term_mode(TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE),
            Some(MouseReport {
                drag: false,
                encoding: Encoding::Sgr
            })
        );
        assert_eq!(
            MouseReport::from_term_mode(TermMode::MOUSE_DRAG | TermMode::UTF8_MOUSE),
            Some(MouseReport {
                drag: true,
                encoding: Encoding::Utf8
            })
        );
        assert_eq!(
            MouseReport::from_term_mode(TermMode::MOUSE_MOTION),
            Some(MouseReport {
                drag: true,
                encoding: Encoding::X10
            })
        );
    }

    #[test]
    fn 器のフラグを読む() {
        // claude（1002 + SGR）
        let f = TmuxMouseFlags::parse("1,1,0,1,0,0\n");
        assert_eq!(f.report(), Some(SGR_DRAG));
        assert!(!f.in_mode);
        // シェル
        assert_eq!(TmuxMouseFlags::parse("0,0,0,0,0,0").report(), None);
        // copy mode
        assert!(TmuxMouseFlags::parse("0,0,0,0,0,1").in_mode);
        // 器が知らないフォーマットは空欄 = false（隣へずれない）
        let f = TmuxMouseFlags::parse("1,,,,,");
        assert_eq!(
            f.report(),
            Some(MouseReport {
                drag: false,
                encoding: Encoding::X10
            })
        );
    }

    fn ctx() -> ClickContext {
        ClickContext {
            enabled: true,
            ..ClickContext::default()
        }
    }

    #[test]
    fn 修飾_フォーカス_遡り_設定ではtakoが取る() {
        assert_eq!(route(&ctx()), ClickRoute::Forward);
        let cases = [
            (
                ClickContext {
                    enabled: false,
                    ..ctx()
                },
                TakoReason::Disabled,
            ),
            (
                ClickContext {
                    legacy: true,
                    ..ctx()
                },
                TakoReason::Legacy,
            ),
            (
                ClickContext {
                    link_modifier: true,
                    ..ctx()
                },
                TakoReason::LinkModifier,
            ),
            (
                ClickContext {
                    bypass_modifier: true,
                    ..ctx()
                },
                TakoReason::BypassModifier,
            ),
            (
                ClickContext {
                    window_activation: true,
                    ..ctx()
                },
                TakoReason::WindowActivation,
            ),
            (
                ClickContext {
                    focus_change: true,
                    ..ctx()
                },
                TakoReason::FocusChange,
            ),
            (
                ClickContext {
                    scrolled_back: true,
                    ..ctx()
                },
                TakoReason::ScrolledBack,
            ),
        ];
        for (c, want) in cases {
            assert_eq!(route(&c), ClickRoute::Tako(want), "{c:?}");
        }
        // A/B は設定 on でも旧挙動（設定より先に見る）
        assert_eq!(
            route(&ClickContext {
                legacy: true,
                link_modifier: true,
                ..ctx()
            }),
            ClickRoute::Tako(TakoReason::Legacy)
        );
    }

    #[test]
    fn 修飾はoptionとshift() {
        assert!(bypass_modifier(true, false));
        assert!(bypass_modifier(false, true));
        assert!(!bypass_modifier(false, false));
    }

    #[test]
    fn 要求していないアプリとcopy_modeには渡さない() {
        assert_eq!(decide(None, false), Err(TakoReason::NotRequested));
        assert_eq!(decide(Some(SGR_DRAG), true), Err(TakoReason::CopyMode));
        assert_eq!(decide(Some(SGR_DRAG), false), Ok(SGR_DRAG));
        assert!(not_forwarded_message(TakoReason::NotRequested).contains("not_requested"));
    }

    #[test]
    fn 移動のレート制限はバースト後に捨てて時間で戻る() {
        let t0 = std::time::Instant::now();
        let mut b = MotionBudget::new(t0);
        let burst = crate::terminal::WHEEL_FORWARD_BURST as usize;
        let taken = (0..burst + 4).filter(|_| b.take(t0)).count();
        assert_eq!(taken, burst, "瞬間はバーストまで");
        // 1 秒で戻る（上限はバースト）
        assert!(b.take(t0 + std::time::Duration::from_secs(1)));
    }

    #[test]
    fn 取り込みビューをバックエンドより優先する() {
        let view = crate::tmux::TmuxView {
            session: "work".into(),
            wrapper: Some("tako-view-1".into()),
            socket: Some("mine".into()),
        };
        assert_eq!(
            TmuxClickSource::for_pane(Some(&view), Some("tako-9"), "tako"),
            Some(TmuxClickSource::Resolved(ScrollTarget::Nested {
                socket: Some("mine".into()),
                session: "tako-view-1".into(),
            }))
        );
        assert_eq!(
            TmuxClickSource::for_pane(None, Some("tako-9"), "tako"),
            Some(TmuxClickSource::Backend {
                socket: "tako".into(),
                session: "tako-9".into(),
            })
        );
        assert_eq!(TmuxClickSource::for_pane(None, None, "tako"), None);
    }

    #[test]
    fn aiのクリックは押すと離すの組() {
        assert_eq!(
            click_pair(SGR_DRAG, 2, 3).unwrap(),
            b"\x1b[<0;3;4M\x1b[<0;3;4m".to_vec()
        );
        let x10 = MouseReport {
            drag: false,
            encoding: Encoding::X10,
        };
        assert!(click_pair(x10, 300, 0).is_err());
    }
}

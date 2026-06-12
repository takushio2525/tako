//! tako-app — GPUI バイナリ（UI 層）
//!
//! Phase 1 後半: 複数ペイン同時描画（PaneTree::layout → GPUI 要素変換）、タブバー、
//! iTerm2 踏襲のキーバインド、色 / カーソル / スクロールバック / 選択コピペ / PTY リサイズ追従。
//!
//! GPUI への依存はこのクレートだけに閉じ込める（`.agent/architecture.md`）。
//! ドメインロジック（Workspace / PaneTree / TerminalSession / Theme / Screen）はすべて
//! tako-core 側にあり、UI 層はドメイン API を呼んで結果を描画するだけにする（設計原則 5:
//! 同じ API を将来 MCP / CLI からも呼ぶため、UI に閉じたロジックを作らない）。
//!
//! `TAKO_SELF_TEST=1` で起動すると、キーディスパッチ経由で入力・分割・タブ・色・
//! スクロールバック・コピペの経路に加え、Phase 2 の制御プレーン（環境変数注入・
//! IPC・`tako` CLI の e2e）と Phase 3 の内蔵 MCP サーバー（Streamable HTTP +
//! stdio ブリッジ）、Phase 3.5 の IME 変換状態（marked text）を機械検証して終了する。

mod autorename;
mod filetree;
mod preview;

use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::{
    actions, canvas, div, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem,
    Context, CursorStyle, ElementInputHandler, EntityInputHandler, FocusHandle, Font, FontStyle,
    FontWeight, HighlightStyle, Hsla, KeyBinding, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, ScrollDelta,
    ScrollWheelEvent, SharedString, Size, StrikethroughStyle, StyledText, TextRun, TextStyle,
    UTF16Selection, UnderlineStyle, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;
use tako_control::{ControlHost, IncomingRequest, IpcServer, McpServer};
use tako_core::{
    ratio_for_position, CommandState, Pane, PaneId, PaneOrigin, Rect, SelectionKind, SessionNotice,
    SpawnOptions, SplitAxis, SplitDirection, TabId, TerminalSession, Theme, TitleSource, Workspace,
};

/// 新規セッションの初期グリッド。最初の render で実寸へリサイズされる
const INITIAL_COLS: usize = 80;
const INITIAL_ROWS: usize = 24;

/// タブバーの高さ（px）
const TAB_BAR_HEIGHT: f32 = 32.0;
/// ペイン枠線の太さ（px）
const PANE_BORDER: f32 = 1.0;
/// ペイン内側の余白（px）
const PANE_PADDING: f32 = 4.0;
/// キーボードリサイズ 1 回あたりの比率変化
const RESIZE_STEP: f32 = 0.05;
/// ペイン境界のドラッグ判定/カーソル変更の当たり幅（px。仕切り線を中心に左右各 BORDER_HANDLE/2）
const BORDER_HANDLE: f32 = 8.0;

/// スクロールバーの当たり領域の幅（px。サムはこの内側に描く）
const SCROLLBAR_WIDTH: f32 = 10.0;
/// スクロールバーの表示維持時間とフェード時間（iTerm2 流の出し方。FR-2.5.13）
const SCROLLBAR_SHOW_MS: u128 = 1000;
const SCROLLBAR_FADE_MS: u128 = 400;

/// 最終スクロールからの経過時間 → スクロールバー不透明度（1.0 → 0.0）
fn scrollbar_alpha(elapsed_ms: u128) -> f32 {
    if elapsed_ms <= SCROLLBAR_SHOW_MS {
        1.0
    } else if elapsed_ms >= SCROLLBAR_SHOW_MS + SCROLLBAR_FADE_MS {
        0.0
    } else {
        1.0 - (elapsed_ms - SCROLLBAR_SHOW_MS) as f32 / SCROLLBAR_FADE_MS as f32
    }
}

/// バックエンド / ネスト tmux スクロールの UI 側状態（ペイン単位）。
/// ホイールは pending に溜めて 1 つの tmux 操作へコアレッシングする
/// （SGR イベント洪水による「ばっと飛ぶ」スクロールの対策。2026-06-12）
struct ScrollCtl {
    /// 解決済みのスクロール実体（None = 未解決。初回ポンプで解決する）
    target: Option<tako_core::scroll::ScrollTarget>,
    state: tako_core::scroll::ScrollState,
    /// 未送信の相対行数（正 = 遡る）
    pending: i32,
    /// スクロールバードラッグの絶対位置目標（pending より優先）
    drag_goal: Option<usize>,
    /// tmux 操作の実行中（完了時に残りをポンプ）
    in_flight: bool,
    /// copy-mode 中の外部変化（ユーザー操作・新規出力）への追従要求
    want_refresh: bool,
    last_activity: std::time::Instant,
    last_refresh: std::time::Instant,
    /// 直近のホイール座標（マウス要求アプリと判明したとき生 SGR へ流す用）
    last_cell: (usize, usize),
}

impl Default for ScrollCtl {
    fn default() -> Self {
        Self {
            target: None,
            state: tako_core::scroll::ScrollState::default(),
            pending: 0,
            drag_goal: None,
            in_flight: false,
            want_refresh: false,
            last_activity: std::time::Instant::now(),
            last_refresh: std::time::Instant::now(),
            last_cell: (0, 0),
        }
    }
}

/// 左サイドバー（ファイルツリー）の幅（px。FR-3.1）
const SIDEBAR_WIDTH: f32 = 220.0;

/// 右サイドバー（情報パネル）の既定幅・最小幅（px。ドラッグで可変）
const PANEL_DEFAULT_WIDTH: f32 = 340.0;
const PANEL_MIN_WIDTH: f32 = 220.0;

/// ペイン上部タイトルバーの高さ（px。iTerm2 風: × ボタン + ペイン名）
const PANE_TITLE_BAR: f32 = 22.0;

/// 下部ステータスバーの高さ（px。FR-2.16.4。Zed / VSCode 風）
const STATUS_BAR_HEIGHT: f32 = 24.0;

/// 右サイドバー情報パネルの内部タブ（固定タブ 0 個方針。2026-06-12）。
/// FR-2.16.6 で agents は tmux ビューへ統合済み。Git は git graph（FR-3.6）の
/// 実装までプレースホルダ（パネルは切り替え式コンテナとして設計）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PanelView {
    #[default]
    Tmux,
    Git,
}

actions!(
    tako,
    [
        SplitRight,
        SplitDown,
        ClosePane,
        NewTab,
        NextTab,
        PrevTab,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        WidenPane,
        NarrowPane,
        TallenPane,
        ShortenPane,
        CopySelection,
        PasteClipboard,
        ToggleSidebar,
        Quit,
        ActivateTab1,
        ActivateTab2,
        ActivateTab3,
        ActivateTab4,
        ActivateTab5,
        ActivateTab6,
        ActivateTab7,
        ActivateTab8,
        ActivateTab9
    ]
);

/// iTerm2 の操作感を踏襲したキーバインド
fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-d", SplitRight, None),
        KeyBinding::new("cmd-shift-d", SplitDown, None),
        KeyBinding::new("cmd-w", ClosePane, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PrevTab, None),
        KeyBinding::new("cmd-alt-left", FocusLeft, None),
        KeyBinding::new("cmd-alt-right", FocusRight, None),
        KeyBinding::new("cmd-alt-up", FocusUp, None),
        KeyBinding::new("cmd-alt-down", FocusDown, None),
        KeyBinding::new("ctrl-cmd-right", WidenPane, None),
        KeyBinding::new("ctrl-cmd-left", NarrowPane, None),
        KeyBinding::new("ctrl-cmd-down", TallenPane, None),
        KeyBinding::new("ctrl-cmd-up", ShortenPane, None),
        KeyBinding::new("cmd-c", CopySelection, None),
        KeyBinding::new("cmd-v", PasteClipboard, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-1", ActivateTab1, None),
        KeyBinding::new("cmd-2", ActivateTab2, None),
        KeyBinding::new("cmd-3", ActivateTab3, None),
        KeyBinding::new("cmd-4", ActivateTab4, None),
        KeyBinding::new("cmd-5", ActivateTab5, None),
        KeyBinding::new("cmd-6", ActivateTab6, None),
        KeyBinding::new("cmd-7", ActivateTab7, None),
        KeyBinding::new("cmd-8", ActivateTab8, None),
        KeyBinding::new("cmd-9", ActivateTab9, None),
    ]
}

fn rgba(c: tako_core::Rgb) -> Rgba {
    Rgba {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: 1.0,
    }
}

fn hsla(c: tako_core::Rgb) -> Hsla {
    rgba(c).into()
}

/// 半透明色（スクロールバー等のオーバーレイ用）
fn rgba_alpha(c: tako_core::Rgb, a: f32) -> Rgba {
    Rgba { a, ..rgba(c) }
}

/// 経過秒の相対表記（tmuxview の作成日時表示用）
fn format_age(seconds: i64) -> String {
    match seconds.max(0) {
        s if s < 60 => format!("{s} 秒前"),
        s if s < 3600 => format!("{} 分前", s / 60),
        s if s < 86400 => format!("{} 時間前", s / 3600),
        s => format!("{} 日前", s / 86400),
    }
}

fn hsla_alpha(c: tako_core::Rgb, a: f32) -> Hsla {
    rgba_alpha(c, a).into()
}

/// コマンド実行状態のラベル（自動リネームの素材・指紋用。list の表現と揃える）
fn command_state_label(state: CommandState) -> &'static str {
    match state {
        CommandState::Unknown => "unknown",
        CommandState::Idle => "idle",
        CommandState::Running => "running",
        CommandState::Failed(_) => "failed",
    }
}

/// 自動リネーム（FR-2.12.4）の起動時の有効判定。
/// セルフテストでは検知ループを止める（トグル・適用経路だけを機械検証する）。
/// `TAKO_AUTO_RENAME=0|false|off` は設定ファイルより優先して無効化する
fn initial_auto_rename() -> bool {
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return false;
    }
    if matches!(
        std::env::var("TAKO_AUTO_RENAME").ok().as_deref(),
        Some("0" | "false" | "off")
    ) {
        return false;
    }
    tako_control::settings::load().auto_rename
}

/// listen ポート検知（FR-2.4.4）の起動時の有効判定。
/// セルフテストでは検知経路そのものを機械検証するため常に有効で始める
fn initial_port_detect() -> bool {
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return true;
    }
    if matches!(
        std::env::var("TAKO_PORT_DETECT").ok().as_deref(),
        Some("0" | "false" | "off")
    ) {
        return false;
    }
    tako_control::settings::load().port_detect
}

/// tmux バックエンド永続化（Phase 5.5 / FR-5）の起動時の有効判定。
/// セルフテストでは既定 OFF（専用項目が dispatch 経由で ON にして検証する。
/// 既存項目を tmux 経由にしてスクロールバック等の挙動を変えない）。
/// `TAKO_PERSIST=0|false|off` は設定ファイルより優先して無効化する
fn initial_tmux_persist() -> bool {
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return false;
    }
    if matches!(
        std::env::var("TAKO_PERSIST").ok().as_deref(),
        Some("0" | "false" | "off")
    ) {
        return false;
    }
    tako_control::settings::load().tmux_persist
}

/// バックエンドセッション名の払い出し（`tako-<hex12>`）。
/// 乱数ベースなので多重起動・PID 再利用でも過去の残骸セッションと衝突しない
fn new_backend_session_name() -> String {
    let token =
        tako_control::generate_token().unwrap_or_else(|_| format!("{:024x}", std::process::id()));
    let tail = &token[..12.min(token.len())];
    format!("{}{tail}", tako_core::tmux_backend::SESSION_PREFIX)
}

/// プレビューを開く（提案チップ承諾アクションの**差し替え点**）。
/// 当面は外部ブラウザで開き、Phase 5 で Web ビューペイン（FR-3.8）が入ったら
/// ここをペイン生成（`tako_open_url` 相当）へ差し替える
fn open_preview(url: &str) {
    // セルフテスト中に実ブラウザを開かない（チップの状態遷移だけ検証する）
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return;
    }
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        eprintln!("warning: ブラウザを開けない: {e}");
    }
}

/// IME 変換中（未確定文字列 = marked text）の状態（FR-1.9）。
/// 変換開始時のフォーカスペインを保持し、変換途中でフォーカスが移っても確定先がぶれないようにする
struct ImeComposition {
    /// 変換対象のペイン（確定文字列の書き込み先）
    pane: PaneId,
    /// 未確定文字列
    text: String,
    /// IME が注目している文節（`text` 内の UTF-16 コード単位の範囲）。太い下線で強調する
    selected_utf16: Option<Range<usize>>,
}

/// UTF-16 コード単位のオフセットを UTF-8 バイトオフセットへ変換する（範囲外は末尾へ丸める）。
/// NSTextInputClient（macOS の IME プロトコル）は範囲をすべて UTF-16 で渡してくる
fn utf16_to_byte_offset(text: &str, utf16_offset: usize) -> usize {
    let mut utf16 = 0;
    for (byte, c) in text.char_indices() {
        if utf16 >= utf16_offset {
            return byte;
        }
        utf16 += c.len_utf16();
    }
    text.len()
}

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

struct TakoApp {
    workspace: Workspace,
    terminals: HashMap<PaneId, TerminalSession>,
    theme: Theme,
    focus_handle: FocusHandle,
    /// 実測したセル寸法（最初の render で確定）
    cell_size: Option<Size<Pixels>>,
    /// マウス選択中のペイン
    selecting: Option<PaneId>,
    /// 直近 render でのアクティブタブ各ペインのテキスト領域（マウス座標→セル変換用）
    pane_text_areas: Vec<(PaneId, Bounds<Pixels>)>,
    /// Layer 1 IPC サーバー（FR-2.2 の受け口。起動失敗時は None で IPC なし動作）
    ipc: Option<IpcServer>,
    /// Layer 2 内蔵 MCP サーバー（FR-2.3 の受け口。起動失敗時は None で MCP なし動作）
    mcp: Option<McpServer>,
    /// IPC / MCP 共有のセッション認証トークン（FR-2.3.4。ログに出さない）
    token: Option<String>,
    /// dispatch 中に依頼されたセッション起動（GPUI の Context が要るため遅延実行する）
    pending_attach: Vec<(PaneId, SpawnOptions)>,
    /// IME 変換中の未確定文字列（FR-1.9。None = 変換中でない）
    ime: Option<ImeComposition>,
    /// ドラッグ中のペイン境界（None = ドラッグしていない）
    dragging_border: Option<DragBorder>,
    /// スクロールバーをドラッグ中のペイン
    dragging_scrollbar: Option<PaneId>,
    /// ホイール行換算の端数持ち越し（accumulate_scroll）。ペイン close で破棄
    scroll_accum: HashMap<PaneId, f32>,
    /// バックエンド / ネスト tmux スクロールの UI 状態（コアレッシング + フェード表示）
    scroll_ctls: HashMap<PaneId, ScrollCtl>,
    /// フェード再描画・copy-mode 追従ティッカーの稼働中フラグ
    scroll_ticker: bool,
    /// 右サイドバー情報パネル（tmux 一覧 FR-2.13 / 集約センター FR-2.10）の表示状態。
    /// 表示・幅・ビューは dispatch（CLI / MCP の `tako panel`）からも操作できる
    panel_visible: bool,
    /// パネルの表示中ビュー（内部タブ）
    panel_view: PanelView,
    /// パネル幅（px。左端ハンドルのドラッグで可変）
    panel_width: f32,
    /// パネル左端の境界をドラッグ中か
    dragging_panel: bool,
    /// tmux 一覧の最新スナップショット（dispatch の TmuxList 結果。表示は描画側の責務）
    tmux_sessions: Vec<serde_json::Value>,
    /// kill の確認待ち（セッション名, window index, tmux サーバー名）。誤爆防止（FR-2.13.3）。
    /// サーバー名は tako バックエンド（Phase 5.5）のセッションを kill するときに使う
    tmux_pending_kill: Option<(String, Option<u32>, Option<String>)>,
    /// 統合 tmux ビューのペイン行ゴミ箱 → kill 確認待ちのペイン（FR-2.16.7。誤爆防止）
    pending_pane_kill: Option<PaneId>,
    /// 左サイドバーのファイルツリー（FR-3.1 / FR-3.7。cmd+B でトグル）
    filetree: filetree::FileTree,
    /// プレビューペイン（FR-3.2 / FR-3.3）。キーに居るペインはターミナルではなく
    /// ファイル内容（コードハイライト / Markdown レンダリング）を描画する
    previews: HashMap<PaneId, preview::PreviewState>,
    /// タブ・ペイン名の AI 自動リネームの検知状態（FR-2.12。ループは new で張る）
    autorename: autorename::AutoRenamer,
    /// listen ポート検知 + 提案チップの有効状態（FR-2.4.4。dispatch から切替）
    port_detect: bool,
    /// 表示中の提案チップ（FR-2.4.3。新規 listen ポートごとに 1 件）
    port_suggestions: Vec<PortSuggestion>,
    /// 却下済みの (ペイン, ポート)。ポートが消えるまで再提案しない
    dismissed_ports: std::collections::HashSet<(PaneId, u16)>,
    /// tmux バックエンド永続化（Phase 5.5 / FR-5）の有効状態（dispatch から切替）
    tmux_persist: bool,
    /// ペインを保持する tmux バックエンドセッション名（persist 有効時のみ登録される）
    backend_sessions: HashMap<PaneId, String>,
    /// 直近に保存したレイアウトの JSON（変化したときだけ書き込むための比較用）
    last_saved_layout: Option<String>,
    /// OS ウィンドウの現フレーム（render で採取し layout 保存に含める。FR-5）
    window_frame: Option<tako_control::layout::WindowFrame>,
}

/// 提案チップ 1 件分（FR-2.4.3。「localhost:PORT をブラウザで開く？」）
#[derive(Debug, Clone, PartialEq, Eq)]
struct PortSuggestion {
    pane: PaneId,
    port: u16,
    process: String,
}

/// 統合 tmux ビューのペイン 1 行分（FR-2.16.6。旧集約センター FR-2.10 の写し）
#[derive(Debug, Clone)]
struct AgentEntry {
    pane: PaneId,
    /// 表示名（ペイン title / role > OSC タイトル > 既定）
    label: String,
    state: CommandState,
    cwd: Option<String>,
    /// ペインを保持する tmux バックエンドセッション名（Phase 5.5。非永続化ペインは None）
    backend: Option<String>,
}

/// 統合 tmux ビューのタブ 1 枠分（FR-2.16.6。タブ名ラベル付き四角枠 + 全ペイン入れ子）
#[derive(Debug, Clone)]
struct TmuxViewTabGroup {
    tab: TabId,
    title: String,
    rows: Vec<AgentEntry>,
    /// このタブのペイン内で attach 中の外部 tmux セッション（FR-2.16.9）
    sessions: Vec<AttachedTmuxSession>,
}

/// タブ内ペインで attach 中の外部 tmux セッション 1 件分（FR-2.16.9）。
/// 別サーバーのセッション（例: orchestrator の master 用）を tako ペインで
/// `tmux attach` して見ている場合、「管理外」ではなくそのタブの配下として表示する
#[derive(Debug, Clone)]
struct AttachedTmuxSession {
    name: String,
    socket: Option<String>,
    /// attach クライアントを表示している tako ペイン
    pane: u64,
    /// (window index, 表示ラベル)
    windows: Vec<(u32, String)>,
}

/// どのタブにも表示されていない tmux セッション 1 件分（FR-2.16.8）。
/// `orphan_backend` = tako から起動されたが対応ペインを失った残骸（kill 漏れ?）、
/// false = tako 管理外（ユーザーが直接立てた等）
#[derive(Debug, Clone)]
struct UnlistedTmuxSession {
    name: String,
    socket: Option<String>,
    orphan_backend: bool,
    attached: bool,
    created: i64,
    location: String,
    /// (window index, 表示ラベル)
    windows: Vec<(u32, String)>,
}

/// ペイン行の並び順（注目度。エラー > 入力待ち > 実行中 > 不明）
fn state_rank(state: CommandState) -> u8 {
    match state {
        CommandState::Failed(_) => 0,
        CommandState::Idle => 1,
        CommandState::Running => 2,
        CommandState::Unknown => 3,
    }
}

/// ドラッグ中の境界の情報。座標→比率換算に必要な分割領域と軸を握っておく
#[derive(Debug, Clone, Copy)]
struct DragBorder {
    /// `PaneTree::set_split_ratio` に渡す分割インデックス
    index: usize,
    /// 分割の軸（Horizontal = 縦線を左右に、Vertical = 横線を上下に動かす）
    axis: SplitAxis,
    /// 分割領域（ウィンドウ座標 px）。`ratio_for_position` でマウス座標を比率へ換算する
    area: Rect,
}

impl TakoApp {
    fn new(cx: &mut Context<Self>) -> Self {
        // IPC（Layer 1）と MCP（Layer 2）の受け口。最初のセッション起動より前に立てて
        // ルートペインのシェルにも TAKO_SOCKET / TAKO_MCP_URL / TAKO_TOKEN を注入できるようにする。
        // 認証トークンは両者で共有する（FR-2.3.4）
        let (control_tx, mut control_rx) = unbounded::<IncomingRequest>();
        let token = match tako_control::generate_token() {
            Ok(token) => Some(token),
            Err(e) => {
                eprintln!("warning: 認証トークンを生成できない（IPC / MCP は使えない）: {e}");
                None
            }
        };
        let ipc = token.as_ref().and_then(|token| {
            match IpcServer::start(control_tx.clone(), token.clone()) {
                Ok(server) => Some(server),
                Err(e) => {
                    eprintln!("warning: IPC サーバーを起動できない（tako CLI は使えない）: {e}");
                    None
                }
            }
        });
        let mcp =
            token
                .as_ref()
                .and_then(|token| match McpServer::start(control_tx, token.clone()) {
                    Ok(server) => Some(server),
                    Err(e) => {
                        eprintln!(
                        "warning: MCP サーバーを起動できない（エージェント連携は使えない）: {e}"
                    );
                        None
                    }
                });

        // 接続情報をファイルへ永続化（FR-2.2.9）。アプリ再起動後も外部の長寿命プロセス
        // から `tako` CLI が繋ぎ直せるようにする（CLI 側のフォールバック先）
        if let (Some(ipc), Some(token)) = (&ipc, &token) {
            let info = tako_control::discovery::ControlInfo {
                version: 1,
                pid: std::process::id(),
                socket: ipc.endpoint().to_string(),
                token: token.clone(),
                mcp_url: mcp.as_ref().map(|m| m.url().to_string()),
            };
            if let Err(e) = tako_control::discovery::write(&info) {
                eprintln!("warning: 接続情報ファイルを書き出せない（再起動後の外部接続は環境変数頼みになる）: {e}");
            }
        }

        // 再起動からの復元（Phase 5.5 / FR-5）。persist 有効 + tmux ありなら
        // レイアウトファイルから同じ ID・同じ構成でワークスペースを再現し、
        // 各ペインは下の spawn ループで tmux バックエンドセッションへ再 attach する
        let tmux_persist = initial_tmux_persist();
        if tmux_persist && tako_core::tmux_backend::available() {
            // 生き残っている既存サーバーへ最新 conf を再適用する（conf は
            // サーバー起動時にしか読まれないため、バージョン更新の設定変更が
            // ここで同期されないと永久に届かない）
            tako_core::tmux_backend::sync_conf(&tako_core::tmux_backend::socket_name());
        }
        let mut restored: Vec<tako_control::layout::RestoredPane> = Vec::new();
        let workspace = if tmux_persist && tako_core::tmux_backend::available() {
            tako_control::layout::load()
                .and_then(|file| tako_control::layout::restore(&file))
                .map(|(ws, panes)| {
                    restored = panes;
                    ws
                })
                .unwrap_or_else(|| Workspace::new("1", Pane::new(PaneOrigin::User)))
        } else {
            Workspace::new("1", Pane::new(PaneOrigin::User))
        };

        let mut app = Self {
            // ルートペイン（復元時は全ペイン）は下の spawn_session でセッションを張る
            workspace,
            terminals: HashMap::new(),
            theme: Theme::default(),
            focus_handle: cx.focus_handle(),
            cell_size: None,
            selecting: None,
            pane_text_areas: Vec::new(),
            ipc,
            mcp,
            token,
            pending_attach: Vec::new(),
            ime: None,
            dragging_border: None,
            dragging_scrollbar: None,
            scroll_accum: HashMap::new(),
            scroll_ctls: HashMap::new(),
            scroll_ticker: false,
            panel_visible: false,
            panel_view: PanelView::default(),
            panel_width: PANEL_DEFAULT_WIDTH,
            dragging_panel: false,
            tmux_sessions: Vec::new(),
            tmux_pending_kill: None,
            pending_pane_kill: None,
            filetree: filetree::FileTree::default(),
            previews: HashMap::new(),
            autorename: autorename::AutoRenamer::new(initial_auto_rename()),
            port_detect: initial_port_detect(),
            port_suggestions: Vec::new(),
            dismissed_ports: std::collections::HashSet::new(),
            tmux_persist,
            backend_sessions: HashMap::new(),
            last_saved_layout: None,
            window_frame: None,
        };
        if restored.is_empty() {
            let root_id = app.workspace.active_tab().tree().focused();
            if let Err(e) = app.spawn_session(root_id, SpawnOptions::default(), cx) {
                // 最初のペインすら開けない環境では使いようがない。SIGABRT ではなく明示終了する
                eprintln!("fatal: 最初のシェルを起動できない: {e}");
                std::process::exit(1);
            }
        } else {
            // 復元 spawn: 保存済みセッション名で attach（消えていれば保存 cwd で開き直し）。
            // セッション名の無かったペイン（直接 spawn 時代）も以後はバックエンドに乗る
            let pane_ids: Vec<PaneId> = app
                .workspace
                .tabs()
                .iter()
                .flat_map(|t| t.tree().panes().into_iter().map(|p| p.id()))
                .collect();
            for r in &restored {
                let Some(&pane) = pane_ids.iter().find(|p| p.as_u64() == r.pane) else {
                    continue;
                };
                // プレビューペイン（FR-3.2）はファイルを開き直すだけ（PTY は起動しない）
                if let Some(p) = &r.preview {
                    let mode = match p.mode.as_str() {
                        "markdown" => preview::PreviewMode::Markdown,
                        _ => preview::PreviewMode::Code,
                    };
                    app.previews
                        .insert(pane, preview::load(std::path::Path::new(&p.path), mode));
                    continue;
                }
                if let Some(name) = &r.session {
                    app.backend_sessions.insert(pane, name.clone());
                }
                let options = SpawnOptions {
                    cwd: r
                        .cwd
                        .clone()
                        .map(std::path::PathBuf::from)
                        .filter(|p| p.is_dir()),
                    ..SpawnOptions::default()
                };
                if let Err(e) = app.spawn_session(pane, options, cx) {
                    eprintln!("warning: ペイン {pane} を復元できない: {e}");
                }
            }
            if app.terminals.is_empty() && app.previews.is_empty() {
                eprintln!("fatal: 復元したペインを 1 つも起動できない");
                std::process::exit(1);
            }
        }

        // IPC リクエストを UI スレッドで dispatch するループ。
        // 操作セマンティクスは tako-control::dispatch に一元化されている（設計原則 5）
        cx.spawn(async move |this, cx| {
            while let Some(incoming) = control_rx.next().await {
                let result = this.update(cx, |app: &mut TakoApp, cx| {
                    let was_scroll = matches!(
                        incoming.request,
                        tako_control::protocol::Request::Scroll { .. }
                    );
                    let mut result = tako_control::dispatch(app, incoming.request, incoming.origin);
                    // CLI / MCP のスクロールでも UI のスクロールバー・カーソル抑止が
                    // 同じ状態を共有する（開発不変条件: UI と AI 操作の等価性）
                    if was_scroll {
                        if let Ok(value) = &result {
                            app.sync_scroll_from_dispatch(value, cx);
                        }
                    }
                    // dispatch が依頼したセッション起動をここで実行（Context が要るため）。
                    // PTY 起動失敗は生成済みペインを巻き戻してエラー応答にする（落とさない）
                    for (pane, options) in std::mem::take(&mut app.pending_attach) {
                        if let Err(e) = app.spawn_session(pane, options, cx) {
                            app.remove_pane(pane, cx);
                            result = Err(tako_control::DispatchError::Operation(format!(
                                "PTY を起動できなかった: {e}"
                            )));
                        }
                    }
                    // AI / CLI 操作によるレイアウト変化を即座に永続化する（Phase 5.5）
                    app.save_layout();
                    cx.notify();
                    result
                });
                match result {
                    // 接続が先に切れていても無視してよい
                    Ok(result) => {
                        let _ = incoming.reply.send(result);
                    }
                    Err(_) => break, // View が破棄された
                }
            }
        })
        .detach();

        // パネルの tmux 一覧表示中は 2 秒毎に更新する（FR-2.13。非表示中は何もしない）。
        // ファイルツリー（FR-3.1）の内容追従も同じ間隔で行う
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let result = this.update(cx, |app: &mut TakoApp, cx| {
                if app.panel_visible && app.panel_view == PanelView::Tmux {
                    app.refresh_tmux(cx);
                }
                app.sync_filetree_roots();
                if app.filetree.visible && app.filetree.refresh() {
                    cx.notify();
                }
                // UI 操作・cwd 変化によるレイアウト変化の定期保存（Phase 5.5。差分時のみ書く）
                app.save_layout();
            });
            if result.is_err() {
                break; // View が破棄された
            }
        })
        .detach();

        // タブ・ペイン名の AI 自動リネーム（FR-2.12）。素材指紋の静穏（デバウンス）を
        // 検知したタブだけ、バックグラウンドで名前生成（claude / ヒューリスティック）を
        // 走らせて反映する。判断はプロンプト 1 本（autorename モジュール）に閉じる
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(autorename::POLL_INTERVAL)
                .await;
            let Ok(jobs) = this.update(cx, |app: &mut TakoApp, _| app.autorename_jobs()) else {
                break; // View が破棄された
            };
            for materials in jobs {
                let tab = materials.tab;
                let plan = cx
                    .background_executor()
                    .spawn(async move { autorename::generate(&materials) })
                    .await;
                if this
                    .update(cx, |app: &mut TakoApp, cx| {
                        app.apply_rename_plan(tab, &plan, cx)
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        // listen ポート検知（FR-2.4.2）。ペインの tty とプロセスの制御端末を突き合わせ、
        // 配下の LISTEN 中 TCP ポートを 3 秒毎に拾って list / MCP へ公開し、
        // 新規ポートには提案チップを立てる（FR-2.4.3）。スキャンはバックグラウンドで行う
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let Ok(ttys) = this.update(cx, |app: &mut TakoApp, _| {
                if !app.port_detect {
                    return Vec::new(); // 無効中（FR-2.4.4）は何もしない
                }
                app.terminals
                    .iter()
                    .filter_map(|(pane, session)| {
                        let rdev = tako_core::ports::tty_rdev(session.tty_name()?)?;
                        Some((*pane, rdev))
                    })
                    .collect::<Vec<_>>()
            }) else {
                break; // View が破棄された
            };
            if ttys.is_empty() {
                continue;
            }
            let rdevs: Vec<u64> = ttys.iter().map(|(_, rdev)| *rdev).collect();
            let mut scanned = cx
                .background_executor()
                .spawn(async move { tako_core::ports::scan(&rdevs) })
                .await;
            let result = this.update(cx, |app: &mut TakoApp, cx| {
                // スキャン中に無効化（portdetect off）が割り込んだら結果を破棄する
                // （クリア済みの listen_ports / チップを古い結果で再汚染しない）
                if !app.port_detect {
                    return;
                }
                let mut changed = false;
                for (pane, rdev) in &ttys {
                    let Some(session) = app.terminals.get_mut(pane) else {
                        continue;
                    };
                    let ports = scanned.remove(rdev).unwrap_or_default();
                    let old: std::collections::HashSet<u16> =
                        session.listen_ports().iter().map(|p| p.port).collect();
                    if !session.set_listen_ports(ports) {
                        continue;
                    }
                    changed = true;
                    let now: Vec<(u16, String)> = session
                        .listen_ports()
                        .iter()
                        .map(|p| (p.port, p.process.clone()))
                        .collect();
                    // 消えたポートのチップ・却下記録は掃除（再 listen で再提案される）
                    app.port_suggestions
                        .retain(|s| s.pane != *pane || now.iter().any(|(port, _)| *port == s.port));
                    app.dismissed_ports
                        .retain(|(p, port)| p != pane || now.iter().any(|(q, _)| q == port));
                    // 新規ポート → 提案チップ（FR-2.4.3。表示だけで強制分割はしない）
                    for (port, process) in &now {
                        let fresh = !old.contains(port)
                            && !app.dismissed_ports.contains(&(*pane, *port))
                            && !app
                                .port_suggestions
                                .iter()
                                .any(|s| s.pane == *pane && s.port == *port);
                        if fresh {
                            app.port_suggestions.push(PortSuggestion {
                                pane: *pane,
                                port: *port,
                                process: process.clone(),
                            });
                        }
                    }
                }
                // 閉じられたペインのチップを掃除
                app.port_suggestions
                    .retain(|s| app.terminals.contains_key(&s.pane));
                if changed {
                    cx.notify();
                }
            });
            if result.is_err() {
                break;
            }
        })
        .detach();

        app
    }

    /// 自動リネームの検知 tick（FR-2.12.2）。タブごとの素材指紋を更新し、
    /// 静穏（デバウンス済み）で未処理のタブの素材一式を返す
    fn autorename_jobs(&mut self) -> Vec<autorename::TabMaterials> {
        let snapshot: Vec<(u64, u64)> = self
            .workspace
            .tabs()
            .iter()
            .map(|tab| {
                // 指紋は「節目」だけで取る（cwd / OSC タイトル / 実行状態 / 手動フラグ）。
                // 画面末尾は実行中に毎 tick 変わり静穏にならないため含めない
                let parts: Vec<_> = tab
                    .tree()
                    .panes()
                    .iter()
                    .map(|p| {
                        let session = self.terminals.get(&p.id());
                        (
                            p.id().as_u64(),
                            p.title_source() == TitleSource::Manual,
                            p.role().map(str::to_string),
                            session.and_then(|s| s.title()).map(str::to_string),
                            session
                                .and_then(|s| s.cwd())
                                .map(|c| c.display().to_string()),
                            session
                                .map(|s| command_state_label(s.command_state()))
                                .unwrap_or("none"),
                        )
                    })
                    .collect();
                let manual_tab = tab.title_source() == TitleSource::Manual;
                (
                    tab.id().as_u64(),
                    autorename::fingerprint(&(manual_tab, parts)),
                )
            })
            .collect();
        self.autorename
            .tick(&snapshot, std::time::Instant::now())
            .into_iter()
            .filter_map(|tab| self.collect_rename_materials(tab))
            .collect()
    }

    /// 命名素材の収集（FR-2.12.1 で list に公開している情報 + 画面末尾）。
    /// 手動リネーム済みのペインは除外する（FR-2.12.3）。素材が無いタブは None
    fn collect_rename_materials(&self, tab_id: u64) -> Option<autorename::TabMaterials> {
        let tab = self
            .workspace
            .tabs()
            .iter()
            .find(|t| t.id().as_u64() == tab_id)?;
        let panes: Vec<autorename::PaneMaterials> = tab
            .tree()
            .panes()
            .iter()
            .filter(|p| p.title_source() != TitleSource::Manual)
            .map(|p| {
                let session = self.terminals.get(&p.id());
                autorename::PaneMaterials {
                    pane: p.id().as_u64(),
                    role: p.role().map(str::to_string),
                    osc_title: session.and_then(|s| s.title()).map(str::to_string),
                    cwd: session
                        .and_then(|s| s.cwd())
                        .map(|c| c.display().to_string()),
                    state: session
                        .map(|s| command_state_label(s.command_state()))
                        .unwrap_or("none"),
                    tail: autorename::trim_tail(
                        session.map(|s| s.visible_lines()).unwrap_or_default(),
                    ),
                }
            })
            .collect();
        // 命名の手がかりが何も無い（シェル統合なし + 出力なし）タブは呼び出しを浪費しない
        let has_signal = panes.iter().any(|p| {
            p.osc_title.is_some() || p.cwd.is_some() || p.state != "unknown" || !p.tail.is_empty()
        });
        if panes.is_empty() || !has_signal {
            return None;
        }
        Some(autorename::TabMaterials {
            tab: tab_id,
            rename_tab: tab.title_source() != TitleSource::Manual,
            panes,
        })
    }

    /// 生成された名前の反映。手動リネーム済みは set_title_auto 側が拒否する（FR-2.12.3）
    fn apply_rename_plan(
        &mut self,
        tab_id: u64,
        plan: &autorename::RenamePlan,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self
            .workspace
            .tabs()
            .iter()
            .map(|t| t.id())
            .find(|t| t.as_u64() == tab_id)
        else {
            return; // 生成中に閉じられたタブ
        };
        let tab = self
            .workspace
            .get_tab_mut(tab_id)
            .expect("直前に存在確認済み");
        if let Some(title) = &plan.tab {
            let _ = tab.set_title_auto(title.clone());
        }
        let pane_ids: Vec<PaneId> = tab.tree().panes().iter().map(|p| p.id()).collect();
        for (id, title) in &plan.panes {
            if let Some(pane_id) = pane_ids.iter().find(|p| p.as_u64() == *id) {
                if let Some(pane) = tab.tree_mut().get_mut(*pane_id) {
                    let _ = pane.set_title_auto(title.clone());
                }
            }
        }
        cx.notify();
    }

    /// 提案チップの承諾（FR-2.4.3）。プレビューを開いてチップを畳む
    fn accept_port_suggestion(&mut self, pane: PaneId, port: u16, cx: &mut Context<Self>) {
        self.port_suggestions
            .retain(|s| !(s.pane == pane && s.port == port));
        open_preview(&format!("http://localhost:{port}"));
        cx.notify();
    }

    /// 提案チップの却下。同じポートが listen し続ける間は再提案しない
    fn dismiss_port_suggestion(&mut self, pane: PaneId, port: u16, cx: &mut Context<Self>) {
        self.port_suggestions
            .retain(|s| !(s.pane == pane && s.port == port));
        self.dismissed_ports.insert((pane, port));
        cx.notify();
    }

    /// 統合 tmux ビューのタブ枠一覧（FR-2.16.6。データと表示の分離 = FR-2.13.5）。
    /// 各枠内のペイン行は注目度順（エラー > 入力待ち > 実行中 > 不明）。
    /// 素材は list（CLI / MCP）と同じ
    fn tmux_view_groups(&self) -> Vec<TmuxViewTabGroup> {
        self.workspace
            .tabs()
            .iter()
            .map(|tab| {
                let mut rows: Vec<AgentEntry> = tab
                    .tree()
                    .panes()
                    .iter()
                    .map(|p| {
                        let session = self.terminals.get(&p.id());
                        let label = match (p.title(), p.role()) {
                            (Some(t), Some(r)) => format!("{t} · {r}"),
                            (Some(t), None) => t.to_string(),
                            (None, Some(r)) => r.to_string(),
                            // プレビューペイン（FR-3.2）はファイル名で表す
                            (None, None) => match self.previews.get(&p.id()) {
                                Some(preview) => format!("📄 {}", preview.file_name()),
                                None => session
                                    .and_then(|s| s.title())
                                    .unwrap_or("シェル")
                                    .to_string(),
                            },
                        };
                        AgentEntry {
                            pane: p.id(),
                            label,
                            state: session
                                .map(|s| s.command_state())
                                .unwrap_or(CommandState::Unknown),
                            cwd: session
                                .and_then(|s| s.cwd())
                                .map(|c| c.display().to_string()),
                            backend: self.backend_sessions.get(&p.id()).cloned(),
                        }
                    })
                    .collect();
                rows.sort_by_key(|e| state_rank(e.state));
                TmuxViewTabGroup {
                    tab: tab.id(),
                    title: tab.title().to_string(),
                    rows,
                    sessions: self.tmux_sessions_attached_to(tab.id().as_u64()),
                }
            })
            .collect()
    }

    /// tmux セッションの attach クライアントが tako ペインで表示中なら
    /// その (tab, pane) を返す（TmuxList が tty 突き合わせ済みの clients を使う）。
    /// tako 自身のバックエンド保持セッションは対象外（タブ枠のペイン行が代表する）
    fn tmux_session_attached_at(session: &serde_json::Value) -> Option<(u64, u64)> {
        if session["backend"].as_bool().unwrap_or(false)
            && session["backend_pane"].as_u64().is_some()
        {
            return None;
        }
        session["clients"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|c| Some((c["tab"].as_u64()?, c["pane"].as_u64()?)))
    }

    /// TmuxList の 1 セッション分 JSON から window 一覧の表示ラベルを組む
    fn tmux_session_windows(session: &serde_json::Value) -> Vec<(u32, String)> {
        session["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| {
                let index = w["index"].as_u64().unwrap_or(0) as u32;
                let label = format!(
                    "{}:{}（{} ペイン）",
                    index,
                    w["name"].as_str().unwrap_or("?"),
                    w["panes"].as_u64().unwrap_or(0),
                );
                (index, label)
            })
            .collect()
    }

    /// 指定タブのペイン内で attach 中の外部 tmux セッション（FR-2.16.9）
    fn tmux_sessions_attached_to(&self, tab: u64) -> Vec<AttachedTmuxSession> {
        self.tmux_sessions
            .iter()
            .filter_map(|session| {
                let (at_tab, pane) = Self::tmux_session_attached_at(session)?;
                if at_tab != tab {
                    return None;
                }
                Some(AttachedTmuxSession {
                    name: session["name"].as_str().unwrap_or("?").to_string(),
                    socket: session["socket"].as_str().map(str::to_string),
                    pane,
                    windows: Self::tmux_session_windows(session),
                })
            })
            .collect()
    }

    /// どのタブにも表示されていない tmux セッションの抽出（FR-2.16.8）。
    /// 対応ペインを持つバックエンドセッションはタブ枠内のペイン行が、tako ペイン内で
    /// attach 中のセッションはタブ枠内の紐付け表示（FR-2.16.9）が代表するため除外し、
    /// 残りを「kill 漏れ?（orphan バックエンド）」と「管理外（ユーザー直起動等）」に分類する
    fn tmux_unlisted_sessions(&self) -> Vec<UnlistedTmuxSession> {
        self.tmux_sessions
            .iter()
            .filter_map(|session| {
                let backend = session["backend"].as_bool().unwrap_or(false);
                if backend && session["backend_pane"].as_u64().is_some() {
                    return None; // タブ枠内のペイン行で表示済み
                }
                if Self::tmux_session_attached_at(session).is_some() {
                    return None; // tako ペイン内で attach 中 = タブ枠へ紐付け表示済み
                }
                let clients: Vec<String> = session["clients"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| match (c["tab"].as_u64(), c["pane"].as_u64()) {
                        (Some(tab), Some(pane)) => format!("tako タブ {tab} / ペイン {pane}"),
                        _ => format!("tako 外（{}）", c["tty"].as_str().unwrap_or("?")),
                    })
                    .collect();
                let location = if backend {
                    "orphan（対応ペインなし）".to_string()
                } else if clients.is_empty() {
                    "detached（どこにも表示されていない）".to_string()
                } else {
                    clients.join("、")
                };
                let windows = Self::tmux_session_windows(session);
                Some(UnlistedTmuxSession {
                    name: session["name"].as_str().unwrap_or("?").to_string(),
                    socket: session["socket"].as_str().map(str::to_string),
                    orphan_backend: backend,
                    attached: session["attached"].as_bool().unwrap_or(false),
                    created: session["created"].as_i64().unwrap_or(0),
                    location,
                    windows,
                })
            })
            .collect()
    }

    /// 集約センターからのジャンプ（FR-2.10.2）。CLI / MCP と同じコマンド層
    /// （dispatch の Focus = タブ切替も伴う）を通す。パネルは開いたまま
    fn jump_to_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::Focus {
                pane: Some(pane.as_u64()),
                direction: None,
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ペインへ移動できない: {e}");
        }
        cx.notify();
    }

    /// tmux 一覧を更新する。UI も CLI / MCP と同じコマンド層（dispatch）を通す
    fn refresh_tmux(&mut self, cx: &mut Context<Self>) {
        self.refresh_tmux_data();
        cx.notify();
    }

    /// tmux 一覧の取得だけ（再描画通知なし。dispatch 内から呼べる形）
    fn refresh_tmux_data(&mut self) {
        let value = tako_control::dispatch(
            self,
            tako_control::protocol::Request::TmuxList { socket: None },
            PaneOrigin::User,
        )
        .unwrap_or_else(|_| serde_json::json!({ "sessions": [] }));
        self.tmux_sessions = value["sessions"].as_array().cloned().unwrap_or_default();
    }

    /// 統合 tmux ビューのペイン行ゴミ箱の確認済み kill（FR-2.16.7）。
    /// CLI / MCP と同じコマンド層（dispatch の Close）を通す
    fn pane_kill_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.pending_pane_kill.take() else {
            return;
        };
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::Close {
                pane: Some(pane.as_u64()),
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ペインを kill できない: {e}");
        }
        cx.notify();
    }

    /// 確認済みの kill を実行する（kill ボタン → 確認 → ここ）
    fn tmux_kill_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some((session, window, socket)) = self.tmux_pending_kill.take() else {
            return;
        };
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::TmuxKill {
                socket,
                session,
                window,
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: tmux を kill できない: {e}");
        }
        self.refresh_tmux(cx);
    }

    /// ペイン ID に対する新しい TerminalSession を起動し、イベント中継タスクを張る。
    /// 制御プレーンの接続情報を環境変数で注入する（FR-2.1.1）。
    /// 失敗（fd 枯渇等での PTY 生成エラー）は Err で返す。ここで panic すると GPUI の
    /// FFI コールバック境界を越えられず SIGABRT でアプリごと落ちる（2026-06-11 常用報告）
    fn spawn_session(
        &mut self,
        pane_id: PaneId,
        mut options: SpawnOptions,
        cx: &mut Context<Self>,
    ) -> Result<(), tako_core::SessionError> {
        options
            .env
            .push(("TAKO_PANE_ID".into(), pane_id.to_string()));
        if let Some(tab_id) = self.workspace.find_tab_of_pane(pane_id) {
            options.env.push(("TAKO_TAB_ID".into(), tab_id.to_string()));
        }
        if let Some(ipc) = &self.ipc {
            options
                .env
                .push(("TAKO_SOCKET".into(), ipc.endpoint().to_string()));
        }
        if let Some(mcp) = &self.mcp {
            options
                .env
                .push(("TAKO_MCP_URL".into(), mcp.url().to_string()));
        }
        if self.ipc.is_some() || self.mcp.is_some() {
            if let Some(token) = &self.token {
                options.env.push(("TAKO_TOKEN".into(), token.clone()));
            }
        }
        // 明示コマンドはログインシェル経由で実行する（.app の最小 PATH では
        // `tmux attach` や `npm` が直接 exec で見つからない。2026-06-12 リグレッション (7)）
        options.command = options.command.map(tako_core::login_shell_command);
        // tmux バックエンド（Phase 5.5 / FR-5）: persist 有効 + tmux ありなら、シェルを
        // 直接ではなく専用サーバーのセッションとして spawn する。`new-session -A` なので
        // 復元時（既存セッション名）は attach、新規ペインは作成と、同じ経路で済む
        let mut backend_session = None;
        if self.tmux_persist && tako_core::tmux_backend::available() {
            let name = self
                .backend_sessions
                .entry(pane_id)
                .or_insert_with(new_backend_session_name)
                .clone();
            options = tako_core::tmux_backend::wrap_options(
                options,
                &tako_core::tmux_backend::socket_name(),
                &name,
            );
            backend_session = Some(name);
        } else {
            self.backend_sessions.remove(&pane_id);
        }
        let (session, mut rx) = TerminalSession::spawn(INITIAL_COLS, INITIAL_ROWS, options)?;
        self.terminals.insert(pane_id, session);
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let result = this.update(cx, |app: &mut TakoApp, cx| {
                    app.on_term_event(pane_id, event, cx);
                });
                if result.is_err() {
                    break; // View が破棄された
                }
            }
        })
        .detach();
        // バックエンド構成では配下プロセスの制御端末は tmux サーバー側のペイン tty になる。
        // ポート検知（FR-2.4.2）と tmuxview（FR-2.13.2）の突き合わせ先をそちらへ差し替える
        // （セッション起動直後はまだ取れないことがあるためバックグラウンドでリトライ）
        if let Some(name) = backend_session {
            let socket = tako_core::tmux_backend::socket_name();
            cx.spawn(async move |this, cx| {
                for _ in 0..20 {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    let (socket, name) = (socket.clone(), name.clone());
                    let tty = cx
                        .background_executor()
                        .spawn(async move { tako_core::tmux_backend::pane_tty(&socket, &name) })
                        .await;
                    let alive = this.update(cx, |app: &mut TakoApp, _| {
                        match (&tty, app.terminals.get_mut(&pane_id)) {
                            (Some(tty), Some(session)) => {
                                session.set_tty_name(Some(tty.clone()));
                                false // 解決済み → ループ終了
                            }
                            (None, Some(_)) => true, // 未解決 → リトライ
                            (_, None) => false,      // ペインが先に閉じた → 打ち切り
                        }
                    });
                    if !matches!(alive, Ok(true)) {
                        return;
                    }
                }
            })
            .detach();
        }
        Ok(())
    }

    fn on_term_event(
        &mut self,
        pane_id: PaneId,
        event: tako_core::SessionEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.terminals.get_mut(&pane_id) else {
            return;
        };
        match session.process_event(event) {
            Some(SessionNotice::Exited) => self.remove_pane(pane_id, cx),
            Some(SessionNotice::ClipboardStore(text)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Some(SessionNotice::TitleChanged) | None => {}
        }
        cx.notify();
    }

    fn focused_pane(&self) -> PaneId {
        self.workspace.active_tab().tree().focused()
    }

    fn focused_session(&self) -> Option<&TerminalSession> {
        self.terminals.get(&self.focused_pane())
    }

    // --- ペイン操作（ドメイン API の薄い呼び出し。FR-2.5 と同じセマンティクス） ---

    fn split(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        let target = self.focused_pane();
        // 分割元ペインの cwd（OSC 7 通知）を継承する（ローカルに無いパスは無視）
        let options = SpawnOptions {
            cwd: self
                .terminals
                .get(&target)
                .and_then(|s| s.cwd())
                .filter(|p| p.is_dir())
                .map(|p| p.to_path_buf()),
            ..SpawnOptions::default()
        };
        let pane = Pane::new(PaneOrigin::User);
        let pane_id = pane.id();
        if self
            .workspace
            .active_tab_mut()
            .tree_mut()
            .split(target, direction, pane)
            .is_ok()
        {
            if let Err(e) = self.spawn_session(pane_id, options, cx) {
                eprintln!("warning: ペインを開けない: {e}");
                self.remove_pane(pane_id, cx);
            }
        }
        cx.notify();
    }

    /// フォーカス中ペインを閉じる。タブ最後の 1 ペインならタブを閉じ、最後のタブならアプリ終了
    fn close_focused_pane(&mut self, cx: &mut Context<Self>) {
        self.remove_pane(self.focused_pane(), cx);
    }

    /// ペインの × ボタン（FR-1.3 の補助 UI）。CLI / MCP と同じコマンド層（dispatch）を
    /// 通す（開発不変条件の UI 側の一貫性）。「最後のタブの最後の 1 ペイン」は dispatch が
    /// 拒否するため、誤クリックでアプリが終了することはない（終了は cmd+W / cmd+Q のみ）
    fn close_pane_button(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::Close {
                pane: Some(pane_id.as_u64()),
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ペインを閉じられない: {e}");
        }
        cx.notify();
    }

    /// ペインを閉じたときのバックエンドセッション破棄（Phase 5.5）。
    /// **明示 close のときだけ**呼ぶ（アプリ終了経路では呼ばない = セッションが残り永続化）。
    /// シェル exit 由来の close では既にセッションが消えており kill は無害な空振りになる
    fn drop_backend_session(&mut self, pane_id: PaneId) {
        if let Some(name) = self.backend_sessions.remove(&pane_id) {
            let socket = tako_core::tmux_backend::socket_name();
            // UI スレッドを塞がない（tmux 不調時の output() 待ちを避ける）
            std::thread::spawn(move || tako_core::tmux_backend::kill_session(&socket, &name));
        }
    }

    fn remove_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(tab_id) = self
            .workspace
            .tabs()
            .iter()
            .find(|t| t.tree().contains(pane_id))
            .map(|t| t.id())
        else {
            return;
        };
        let tab = self
            .workspace
            .get_tab_mut(tab_id)
            .expect("直前に存在を確認したタブ");
        match tab.tree_mut().close(pane_id) {
            Ok(_) => {
                self.terminals.remove(&pane_id);
                self.previews.remove(&pane_id);
                self.scroll_accum.remove(&pane_id);
                self.scroll_ctls.remove(&pane_id);
                self.drop_backend_session(pane_id);
            }
            Err(_) => {
                // LastPane: タブごと閉じる
                self.remove_tab(tab_id, cx);
            }
        }
        cx.notify();
    }

    fn remove_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let Some(tab) = self.workspace.get_tab(tab_id) else {
            return;
        };
        let pane_ids: Vec<PaneId> = tab.tree().panes().iter().map(|p| p.id()).collect();
        match self.workspace.close_tab(tab_id) {
            Ok(_) => {
                for id in pane_ids {
                    self.terminals.remove(&id);
                    self.previews.remove(&id);
                    self.scroll_accum.remove(&id);
                    self.scroll_ctls.remove(&id);
                    self.drop_backend_session(id);
                }
            }
            Err(_) => {
                // LastTab: アプリ終了は UI 層の責務。最後のペインの明示 close なので
                // セッションも破棄し、次回起動で空レイアウトを復元しないようファイルも消す
                for id in pane_ids {
                    self.drop_backend_session(id);
                }
                if std::env::var_os("TAKO_SELF_TEST").is_none() {
                    tako_control::layout::remove();
                }
                tako_control::discovery::cleanup(std::process::id());
                cx.quit();
            }
        }
        cx.notify();
    }

    /// レイアウトの保存（Phase 5.5 / FR-5）。構造が変わったときだけ書き込む。
    /// 定期ループ（2 秒）・dispatch 後・終了時に呼ばれる。セルフテスト中は
    /// ユーザーのレイアウトファイルを汚さない
    fn save_layout(&mut self) {
        if !self.tmux_persist
            || !tako_core::tmux_backend::available()
            || std::env::var_os("TAKO_SELF_TEST").is_some()
        {
            return;
        }
        let backend_sessions = &self.backend_sessions;
        let terminals = &self.terminals;
        let previews = &self.previews;
        let layout = tako_control::layout::capture(
            &self.workspace,
            &|pane| tako_control::layout::PaneMeta {
                session: backend_sessions.get(&pane).cloned(),
                cwd: terminals
                    .get(&pane)
                    .and_then(|s| s.cwd())
                    .map(|p| p.display().to_string()),
                preview: previews
                    .get(&pane)
                    .map(|p| tako_control::layout::PreviewLayout {
                        path: p.path.display().to_string(),
                        mode: p.mode.to_wire().as_str().to_string(),
                    }),
            },
            self.window_frame.clone(),
        );
        let Ok(json) = serde_json::to_string(&layout) else {
            return;
        };
        if self.last_saved_layout.as_deref() == Some(json.as_str()) {
            return;
        }
        match tako_control::layout::save(&layout) {
            Ok(_) => self.last_saved_layout = Some(json),
            Err(e) => eprintln!("warning: レイアウトを保存できない: {e}"),
        }
    }

    fn focus_direction(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        self.workspace
            .active_tab_mut()
            .tree_mut()
            .focus_direction(direction);
        cx.notify();
    }

    fn resize_focused(&mut self, axis: SplitAxis, delta: f32, cx: &mut Context<Self>) {
        let target = self.focused_pane();
        // 合う軸の分割が無いときは何もしない（単一ペイン等）
        let _ = self
            .workspace
            .active_tab_mut()
            .tree_mut()
            .resize_by(target, axis, delta);
        cx.notify();
    }

    fn new_tab(&mut self, cx: &mut Context<Self>) {
        let title = format!("{}", self.workspace.tabs().len() + 1);
        let pane = Pane::new(PaneOrigin::User);
        let pane_id = pane.id();
        self.workspace.create_tab(title, pane);
        if let Err(e) = self.spawn_session(pane_id, SpawnOptions::default(), cx) {
            eprintln!("warning: タブを開けない: {e}");
            self.remove_pane(pane_id, cx); // 最後の 1 ペイン → タブごと畳まれる
        }
        cx.notify();
    }

    fn activate_tab_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.workspace.tabs().get(index).map(|t| t.id()) {
            let _ = self.workspace.activate_tab(id);
        }
        cx.notify();
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.focused_session().and_then(|s| s.selection_text()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if let Some(session) = self.focused_session() {
            session.paste(&text);
        }
        cx.notify();
    }

    // --- キー入力 ---

    fn handle_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        // cmd を含む未バインドのキーはシェルへ流さない
        if keystroke.modifiers.platform {
            return;
        }
        // tmux バックエンドペインでは修飾付きキーの CSI u 送出を常に有効化する。
        // 内側アプリ（claude 等）が kitty protocol を要求しても tmux は外側端末へ
        // 伝えない（core の e2e で確認済み）ため、外側 Term のモードだけでは
        // Shift+Enter を区別できない。tmux は extended-keys always で CSI u を解釈し
        // 内側ペインへ届けるので、修飾付きキーは常時 CSI u で安全。
        // ただし **Esc 単押しは CSI 27u にしない**（ModifiedOnly）: tmux 3.6 は受信した
        // CSI 27u を内側ペインの kitty 要求の有無に関係なく素通しするため、CSI u
        // 非対応アプリ（素の zsh 等）の入力欄に「27u」が文字として挿入される
        // （2026-06-12 実機バグ）。素の \e は tmux が escape-time で正しく解釈し
        // 内側へ素のまま届く（core e2e で固定）
        let kitty_requested = self
            .focused_session()
            .map(|s| s.disambiguate_keys())
            .unwrap_or(false);
        let csi_u = if kitty_requested {
            CsiUMode::Full
        } else if self.backend_sessions.contains_key(&self.focused_pane()) {
            CsiUMode::ModifiedOnly
        } else {
            CsiUMode::Off
        };
        if let Some(bytes) = keystroke_to_bytes(keystroke, csi_u) {
            // tmux スクロール中（copy-mode）は iTerm2 流に最下部へ戻してから流す
            // （copy-mode にキーが飲まれて「入力が反映されない」症状の根治）
            self.cancel_scroll_before_input(self.focused_pane());
            if let Some(session) = self.focused_session() {
                session.clear_selection();
                session.write(bytes);
                // ここで処理済みを宣言しないと、macOS が未処理キーを IME（input handler）へ
                // 回送し insertText → replace_text_in_range で二重入力になる（FR-1.9）
                cx.stop_propagation();
                cx.notify();
            }
        }
    }

    // --- IME（FR-1.9） ---

    /// IME 操作の対象ペイン。変換中はその開始ペイン、それ以外はフォーカスペイン
    fn ime_target(&self) -> PaneId {
        self.ime
            .as_ref()
            .map(|ime| ime.pane)
            .unwrap_or_else(|| self.focused_pane())
    }

    /// 指定ペインのカーソルセル左上（ウィンドウ座標）。
    /// スクロールバック表示中などカーソル非表示のときは None
    /// ペインのカーソル左上のウィンドウ座標。x は描画と同じ shaping で求める
    /// （`cell_at` の逆写像）。全角行は advance ≠ セル幅 × 2 のため、col × セル幅の
    /// 線形換算だと打ち進めるほど IME 候補ウィンドウ・未確定文字列が右へずれていく
    /// （2026-06-12 実機リグレッション (5) の根本原因）
    fn pane_cursor_origin(&self, pane: PaneId, window: &mut Window) -> Option<Point<Pixels>> {
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane)?;
        let cell = self.cell_size?;
        let screen = self.terminals.get(&pane)?.screen(&self.theme);
        let (col, row) = screen.cursor?;
        let x = match screen.lines.get(row) {
            Some(line) => {
                // カーソル col に対応する文字（cell_cols は単調非減少）の描画位置
                let char_ix = line
                    .cell_cols
                    .iter()
                    .position(|&c| c >= col)
                    .unwrap_or(line.cell_cols.len());
                let byte_ix = line
                    .text
                    .char_indices()
                    .nth(char_ix)
                    .map(|(i, _)| i)
                    .unwrap_or(line.text.len());
                let shaped = window.text_system().shape_line(
                    SharedString::from(line.text.clone()),
                    px(self.theme.font_size),
                    &[self.mono_text_run(line.text.len())],
                    None,
                );
                f32::from(shaped.x_for_index(byte_ix))
            }
            // スナップショット外（起動直後等）は等幅前提の線形換算へフォールバック
            None => f32::from(cell.width) * col as f32,
        };
        Some(point(
            area.origin.x + px(x),
            area.origin.y + cell.height * row as f32,
        ))
    }

    /// 未確定文字列の先頭から指定プレフィックスまでの描画幅（候補ウィンドウの位置出し用）
    fn ime_prefix_width(&self, prefix: &str, window: &mut Window) -> Pixels {
        if prefix.is_empty() {
            return px(0.0);
        }
        let run = TextRun {
            len: prefix.len(),
            font: gpui::font(self.theme.font_family.clone()),
            color: hsla(self.theme.foreground),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(
                SharedString::from(prefix.to_string()),
                px(self.theme.font_size),
                &[run],
                None,
            )
            .width
    }

    // --- マウス ---

    /// ウィンドウ座標をペイン内のセル座標へ変換する（col, row, セル右半分か）
    /// マウス座標 → グリッドセル（col, row, セル内右半分か）。
    /// 行の描画はプロポーショナルな実フォント幅で行われ、全角文字の advance は
    /// セル幅 × 2 と一致しない（描画とグリッドがずれる）。そのため x は描画と同じ
    /// shaping で文字位置へ直し、`ScreenLine::cell_cols` でグリッド col に写像する
    fn cell_at(
        &self,
        pane_id: PaneId,
        position: Point<Pixels>,
        window: &mut Window,
    ) -> Option<(usize, usize, bool)> {
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane_id)?;
        let cell = self.cell_size?;
        let session = self.terminals.get(&pane_id)?;
        let (cols, rows) = session.size();
        let local = position - area.origin;
        let y = (f32::from(local.y) / f32::from(cell.height)).max(0.0);
        let row = (y as usize).min(rows.saturating_sub(1));
        let local_x = f32::from(local.x).max(0.0);

        let screen = session.screen(&self.theme);
        let Some(line) = screen.lines.get(row) else {
            // スナップショット外（起動直後等）は等幅前提の線形換算へフォールバック
            let col = ((local_x / f32::from(cell.width)) as usize).min(cols.saturating_sub(1));
            return Some((col, row, false));
        };
        let shaped = window.text_system().shape_line(
            SharedString::from(line.text.clone()),
            px(self.theme.font_size),
            &[self.mono_text_run(line.text.len())],
            None,
        );
        let byte_ix = shaped.closest_index_for_x(px(local_x));
        let char_ix = line.text[..byte_ix].chars().count();
        let col = line
            .cell_cols
            .get(char_ix)
            .copied()
            // 行テキスト末尾より右は線形換算（テキストは全列を埋めるため通常は来ない）
            .unwrap_or((local_x / f32::from(cell.width)) as usize)
            .min(cols.saturating_sub(1));
        // セル内の左右判定も描画上の文字幅で行う
        let char_start = f32::from(shaped.x_for_index(byte_ix));
        let next_ix = line.text[byte_ix..]
            .chars()
            .next()
            .map(|c| byte_ix + c.len_utf8())
            .unwrap_or(byte_ix);
        let char_end = f32::from(shaped.x_for_index(next_ix));
        let side_right =
            char_end > char_start && (local_x - char_start) / (char_end - char_start) > 0.5;
        Some((col, row, side_right))
    }

    /// kill 確認のインラインブロック（FR-2.16.7）。メッセージ行（折り返し）+ ボタン行の
    /// 縦積みにし、文言が長くてもボタンがパネル右端へ見切れないようにする
    /// （flex_row 一列だと長文時にボタンごと overflow_hidden で切られる。2026-06-13 実機）。
    /// `confirm_pane` = Some ならペイン kill（dispatch Close）、None なら tmux kill
    fn render_kill_confirm(
        &self,
        id_seed: u64,
        message: String,
        confirm_pane: Option<PaneId>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = &self.theme;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pl_4()
            .w_full()
            .text_size(px(11.0))
            .text_color(hsla(theme.ansi[1]))
            .child(div().w_full().child(SharedString::from(message)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id(("kill-yes", id_seed))
                            .px_2()
                            .flex_none()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgba_alpha(theme.ansi[1], 0.25))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if confirm_pane.is_some() {
                                    this.pane_kill_confirmed(cx);
                                } else {
                                    this.tmux_kill_confirmed(cx);
                                }
                            }))
                            .child("kill する"),
                    )
                    .child(
                        div()
                            .id(("kill-no", id_seed))
                            .px_2()
                            .flex_none()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgba_alpha(theme.tab_bar_background, 0.9))
                            .text_color(hsla(theme.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if confirm_pane.is_some() {
                                    this.pending_pane_kill = None;
                                } else {
                                    this.tmux_pending_kill = None;
                                }
                                cx.notify();
                            }))
                            .child("やめる"),
                    ),
            )
    }

    /// 統合 tmux ビュー（FR-2.16.6〜2.16.9。旧 tmuxview FR-2.13 + 集約センター FR-2.10 の
    /// 1 本化）。タブごとの「タブ名ラベル付き四角枠」に全ペインを入れ子表示し、行クリックで
    /// ジャンプ、ゴミ箱 → 確認 → kill（dispatch の Close）。タブ内ペインで attach 中の
    /// 外部 tmux セッションは window 一覧ごとタブ枠へ紐付け表示する（FR-2.16.9）。続けて、
    /// どのタブにも表示されていない tmux セッションを「管理外 / kill 漏れ?」に区別して
    /// 列挙する（確認つき TmuxKill）
    fn render_tmux_view(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let groups = self.tmux_view_groups();
        let unlisted = self.tmux_unlisted_sessions();
        let pending_pane = self.pending_pane_kill;
        let pending_tmux = self.tmux_pending_kill.clone();
        let active_tab = self.workspace.active_tab_id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let mut root = div()
            .id("tmux-view")
            .flex_1()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgba(theme.background))
            .text_color(hsla(theme.foreground))
            .text_size(px(12.0))
            .overflow_y_scroll()
            .child(
                div()
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .text_size(px(11.0))
                    .child("タブごとの全ペイン（クリックでジャンプ。kill は確認つき）"),
            );

        // タブ枠: タブ名ラベル付き四角枠 + 枠内に全ペインの入れ子表示（FR-2.16.6）
        for (group_index, group) in groups.into_iter().enumerate() {
            let is_active = group.tab == active_tab;
            let mut card = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(if is_active {
                    hsla_alpha(theme.accent, 0.6)
                } else {
                    hsla(theme.pane_border)
                })
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_active {
                            hsla(theme.tab_active_foreground)
                        } else {
                            hsla(theme.tab_inactive_foreground)
                        })
                        .overflow_hidden()
                        .child(SharedString::from(format!(
                            "タブ {}",
                            truncate(&group.title, 30)
                        ))),
                );
            for row in group.rows {
                let pane = row.pane;
                let (color, state_label) = match row.state {
                    CommandState::Failed(code) => (theme.ansi[1], format!("エラー ({code})")),
                    CommandState::Idle => (theme.ansi[2], "入力待ち".to_string()),
                    CommandState::Running => (theme.accent, "実行中".to_string()),
                    CommandState::Unknown => {
                        (theme.tab_inactive_foreground, "状態不明".to_string())
                    }
                };
                // 補足（cwd / 保持セッション）は詰めすぎず省略（…）で見切れを防ぐ
                let detail = match (&row.cwd, &row.backend) {
                    (Some(cwd), Some(b)) => {
                        format!("{} ・ tmux: {}", truncate(cwd, 24), truncate(b, 16))
                    }
                    (Some(cwd), None) => truncate(cwd, 36),
                    (None, Some(b)) => format!("tmux: {}", truncate(b, 24)),
                    (None, None) => String::new(),
                };
                card = card.child(
                    div()
                        .id(("tmux-pane-row", pane.as_u64()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .overflow_hidden()
                        .hover(|d| d.bg(rgba_alpha(theme.tab_bar_background, 0.8)))
                        .on_click(cx.listener(move |this, _, _, cx| this.jump_to_pane(pane, cx)))
                        .child(
                            div()
                                .w(px(8.0))
                                .h(px(8.0))
                                .flex_none()
                                .rounded_full()
                                .bg(hsla(color)),
                        )
                        .child(
                            div()
                                .w(px(64.0))
                                .flex_none()
                                .text_size(px(11.0))
                                .text_color(hsla(color))
                                .child(SharedString::from(state_label)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .font_weight(FontWeight::BOLD)
                                .child(SharedString::from(truncate(&row.label, 28))),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .overflow_hidden()
                                .child(SharedString::from(detail)),
                        )
                        .child(
                            // ゴミ箱 → 確認 → kill（FR-2.16.7）
                            div()
                                .id(("pane-kill", pane.as_u64()))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla_alpha(theme.ansi[1], 0.8))
                                .hover(|d| d.bg(rgba_alpha(theme.ansi[1], 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.pending_pane_kill = Some(pane);
                                    cx.notify();
                                }))
                                .child("🗑"),
                        ),
                );
                if pending_pane == Some(pane) {
                    card = card.child(self.render_kill_confirm(
                        pane.as_u64(),
                        format!("ペイン {pane} を kill していいですか?（中のプロセスごと終了）"),
                        Some(pane),
                        cx,
                    ));
                }
            }
            // タブ内ペインで attach 中の外部 tmux セッション（FR-2.16.9）。
            // 「管理外」へ落とさず、見えているタブの配下として window 一覧ごと表示する
            for (s_index, session) in group.sessions.iter().enumerate() {
                // 確認 UI の id 衝突を避ける（ペイン kill は pane id、こちらは上位ビット）
                let id_seed = (1 << 32) | ((group_index as u64) << 16) | s_index as u64;
                let kill_name = session.name.clone();
                let kill_socket = session.socket.clone();
                card = card.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_1()
                        .overflow_hidden()
                        .child(
                            div()
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .text_size(px(10.0))
                                .text_color(hsla(theme.accent))
                                .bg(rgba_alpha(theme.accent, 0.15))
                                .child("tmux"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(11.0))
                                .child(SharedString::from(truncate(&session.name, 24))),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(SharedString::from(format!(
                                    "ペイン {} で attach 中",
                                    session.pane
                                ))),
                        )
                        .child(
                            div()
                                .id(("tmux-att-kill", id_seed))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla_alpha(theme.ansi[1], 0.8))
                                .hover(|d| d.bg(rgba_alpha(theme.ansi[1], 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.tmux_pending_kill =
                                        Some((kill_name.clone(), None, kill_socket.clone()));
                                    cx.notify();
                                }))
                                .child("🗑"),
                        ),
                );
                for (w_index, label) in &session.windows {
                    let w_index = *w_index;
                    let kill_name = session.name.clone();
                    let kill_socket = session.socket.clone();
                    card = card.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .pl_4()
                            .text_size(px(11.0))
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(SharedString::from(truncate(label, 40))),
                            )
                            .child(
                                div()
                                    .id(("tmux-att-kill-window", (id_seed << 8) | w_index as u64))
                                    .px_1()
                                    .flex_none()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_size(px(10.0))
                                    .text_color(hsla_alpha(theme.ansi[1], 0.8))
                                    .hover(|d| d.bg(rgba_alpha(theme.ansi[1], 0.2)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.tmux_pending_kill = Some((
                                            kill_name.clone(),
                                            Some(w_index),
                                            kill_socket.clone(),
                                        ));
                                        cx.notify();
                                    }))
                                    .child("🗑"),
                            ),
                    );
                }
                // attach 済みセッションへの kill 確認（unlisted 側と同じ pending を使う）
                if let Some((pending_session, pending_window, _)) = &pending_tmux {
                    if *pending_session == session.name {
                        let label = match pending_window {
                            Some(w) => format!(
                                "window {w} を kill していいですか?（中のプロセスごと終了）"
                            ),
                            None => format!(
                                "セッション {} を kill していいですか?（中のプロセスごと終了。\
                                 attach 中のペインからも消える）",
                                session.name
                            ),
                        };
                        card = card.child(self.render_kill_confirm(id_seed, label, None, cx));
                    }
                }
            }
            root = root.child(card);
        }

        // どのタブにも表示されていない tmux セッション（FR-2.16.8。
        // 管理外 = ユーザー直起動等 / kill 漏れ? = orphan バックエンドの残骸）
        if !unlisted.is_empty() {
            root = root.child(
                div()
                    .mt_2()
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .text_size(px(11.0))
                    .child("どのタブにも表示されていない tmux セッション（2 秒毎に更新）"),
            );
        }
        for (index, session) in unlisted.iter().enumerate() {
            let (badge_label, badge_color) = if session.orphan_backend {
                ("kill漏れ?", theme.ansi[1]) // 赤: tako が起動して kill し損ねた残骸
            } else {
                ("管理外", theme.ansi[3]) // 黄: tako の外で立てられたセッション
            };
            let kill_name = session.name.clone();
            let kill_socket = session.socket.clone();
            let mut card = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(rgba_alpha(theme.tab_bar_background, 0.6))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .overflow_hidden()
                        .child(
                            div()
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .text_size(px(10.0))
                                .text_color(hsla(badge_color))
                                .bg(rgba_alpha(badge_color, 0.15))
                                .child(badge_label),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .overflow_hidden()
                                .child(SharedString::from(truncate(&session.name, 24))),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .flex_none()
                                .text_color(if session.attached {
                                    hsla(theme.accent)
                                } else {
                                    hsla(theme.ansi[3])
                                })
                                .child(if session.attached {
                                    "attached"
                                } else {
                                    "detached"
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .overflow_hidden()
                                .child(SharedString::from(format!(
                                    "作成 {} ・ {}",
                                    format_age(now - session.created),
                                    session.location,
                                ))),
                        )
                        .child(div().flex_grow(1.0))
                        .child(
                            div()
                                .id(("tmux-kill", index as u64))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla_alpha(theme.ansi[1], 0.8))
                                .hover(|d| d.bg(rgba_alpha(theme.ansi[1], 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tmux_pending_kill =
                                        Some((kill_name.clone(), None, kill_socket.clone()));
                                    cx.notify();
                                }))
                                .child("🗑"),
                        ),
                )
                .children(session.windows.iter().map(|(w_index, label)| {
                    let w_index = *w_index;
                    let kill_name = session.name.clone();
                    let kill_socket = session.socket.clone();
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .pl_4()
                        .text_size(px(11.0))
                        .overflow_hidden()
                        .child(
                            // ラベルは flex_1 で縮められるように包む（裸のテキスト子だと
                            // 長文時にゴミ箱ごと右へ押し出されて見切れる）
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .child(SharedString::from(truncate(label, 40))),
                        )
                        .child(
                            div()
                                .id(("tmux-kill-window", ((index as u64) << 16) | w_index as u64))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(10.0))
                                .text_color(hsla_alpha(theme.ansi[1], 0.8))
                                .hover(|d| d.bg(rgba_alpha(theme.ansi[1], 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tmux_pending_kill = Some((
                                        kill_name.clone(),
                                        Some(w_index),
                                        kill_socket.clone(),
                                    ));
                                    cx.notify();
                                }))
                                .child("🗑"),
                        )
                }));
            // 誤爆防止のインライン確認（FR-2.13.3 / FR-2.16.8）
            if let Some((pending_session, pending_window, _)) = &pending_tmux {
                if *pending_session == session.name {
                    let name = &session.name;
                    let label = match (pending_window, session.orphan_backend) {
                        (Some(w), _) => {
                            format!("window {w} を kill していいですか?（中のプロセスごと終了）")
                        }
                        (None, true) => format!(
                            "{name} は tako の kill 漏れ残骸の可能性。kill していいですか?（中のプロセスごと終了）"
                        ),
                        (None, false) => format!(
                            "管理外セッション {name} を kill していいですか?（中のプロセスごと終了）"
                        ),
                    };
                    // 確認 UI の id は attach 済み側（1<<32 系）と衝突しない下位値
                    card = card.child(self.render_kill_confirm(index as u64, label, None, cx));
                }
            }
            root = root.child(card);
        }
        root
    }

    /// git ビュー（FR-2.16.4 の git トグルの表示先）。git graph（FR-3.6）の実装までの
    /// プレースホルダ
    fn render_git_view(&mut self) -> gpui::Div {
        let theme = self.theme.clone();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .p_4()
            .bg(rgba(theme.background))
            .text_color(hsla(theme.tab_inactive_foreground))
            .text_size(px(12.0))
            .child("git graph は未実装（FR-3.6）")
            .child("ブランチ・コミットのグラフ表示をここに追加予定")
    }

    /// ファイルツリーの root をフォーカスペインの cwd（無ければ $HOME）に追従させる
    /// （FR-3.1。render・トグル・定期ループから呼ばれる。非表示中は何もしない）
    /// 「タブ = ワークスペース」の同期(FR-3.1。2026-06-13 変更): アクティブタブ内の
    /// 全ペインの cwd（OSC 7）をワークスペースフォルダとしてツリーへ並べる。
    /// cwd が 1 つも取れないときはホームへフォールバック
    fn sync_filetree_roots(&mut self) {
        if !self.filetree.visible {
            return;
        }
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        for pane in self.workspace.active_tab().tree().panes() {
            let Some(cwd) = self
                .terminals
                .get(&pane.id())
                .and_then(|s| s.cwd())
                .filter(|p| p.is_dir())
            else {
                continue;
            };
            let cwd = cwd.to_path_buf();
            if !roots.contains(&cwd) {
                roots.push(cwd);
            }
        }
        if roots.is_empty() {
            if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
                roots.push(home);
            }
        }
        self.filetree.set_roots(roots);
    }

    /// 左サイドバーのファイルツリー（FR-3.1。非表示なら None = 純粋なターミナル FR-3.7）。
    /// 「タブ = ワークスペース」: タブ内全ペインの cwd がワークスペースフォルダとして並ぶ
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.filetree.visible {
            return None;
        }
        let theme = self.theme.clone();
        let tab_title = self.workspace.active_tab().title().to_string();
        // プレビュー表示中のファイル（開いている行を控えめにハイライトする）
        let open_paths: std::collections::HashSet<std::path::PathBuf> =
            self.previews.values().map(|p| p.path.clone()).collect();
        let rows = self.filetree.rows();
        Some(
            div()
                .w(px(SIDEBAR_WIDTH))
                .h_full()
                .flex()
                .flex_col()
                .bg(rgba(theme.tab_bar_background))
                .border_r_1()
                .border_color(hsla(theme.pane_border))
                .text_size(px(12.0))
                .text_color(hsla(theme.foreground))
                .overflow_hidden()
                .child(
                    // ヘッダ: ワークスペース = アクティブタブ（VSCode のエクスプローラ相当）
                    div()
                        .px_2()
                        .py_1()
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.9))
                        .child(SharedString::from(format!(
                            "ワークスペース — {}",
                            truncate(&tab_title, 20)
                        ))),
                )
                .child(
                    div()
                        .id("filetree-list")
                        .flex_1()
                        .flex()
                        .flex_col()
                        .overflow_y_scroll()
                        .children(rows.into_iter().enumerate().map(|(index, row)| {
                            let path = row.entry.path.clone();
                            let is_dir = row.entry.is_dir;
                            let is_open = !is_dir && open_paths.contains(&path);
                            let chevron = if is_dir {
                                if row.expanded {
                                    "▾ "
                                } else {
                                    "▸ "
                                }
                            } else {
                                "  "
                            };
                            let base = div()
                                .id(("filetree-row", index as u64))
                                .flex()
                                .flex_row()
                                .items_center()
                                .w_full()
                                .px_1()
                                .cursor_pointer()
                                .hover(|d| d.bg(rgba(theme.tab_active_background)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if is_dir {
                                        this.filetree.toggle_dir(&path);
                                    } else {
                                        this.open_file_row(&path, cx);
                                    }
                                    cx.notify();
                                }));
                            if row.root {
                                // ワークスペースフォルダの見出し行: 太字 + 上仕切り線（2 つ目以降）
                                base.when(index > 0, |d| {
                                    d.border_t_1()
                                        .border_color(hsla_alpha(theme.pane_border, 0.6))
                                        .mt_1()
                                })
                                .py(px(2.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(theme.tab_active_foreground))
                                .child(SharedString::from(
                                    format!("{chevron}🗂 {}", truncate(&row.entry.name, 22)),
                                ))
                            } else {
                                base.pl(px(8.0 + 12.0 * row.depth as f32))
                                    .when(!is_dir, |d| {
                                        d.text_color(hsla_alpha(theme.foreground, 0.85))
                                    })
                                    .when(is_open, |d| {
                                        d.bg(rgba_alpha(theme.tab_active_background, 0.6))
                                            .text_color(hsla(theme.accent))
                                    })
                                    .child(SharedString::from(format!(
                                        "{chevron}{}",
                                        truncate(&row.entry.name, 24)
                                    )))
                            }
                        })),
                ),
        )
    }

    /// ファイルツリーのファイル行クリック → プレビューペインで開く（FR-3.2）。
    /// CLI / MCP（`tako open` / `tako_open_file`）と同じ dispatch 経路を通す
    /// （開発不変条件の UI 側の一貫性。OpenFile はセッション起動を伴わないため
    /// pending_attach の後処理は不要）
    fn open_file_row(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let pane = self.focused_pane().as_u64();
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::OpenFile {
                pane: Some(pane),
                path: path.display().to_string(),
                mode: None,
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ファイルを開けない: {e}");
        }
        cx.notify();
    }

    /// プレビューの「コード ⇔ Markdown」トグル（目アイコン。FR-3.3）。
    /// 同じ状態は dispatch（OpenFile の mode 指定）= CLI / MCP からも切り替えられる
    fn toggle_preview_mode(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(state) = self.previews.get(&pane_id) else {
            return;
        };
        let mode = match state.mode {
            preview::PreviewMode::Code => preview::PreviewMode::Markdown,
            preview::PreviewMode::Markdown => preview::PreviewMode::Code,
        };
        let path = state.path.clone();
        self.previews.insert(pane_id, preview::load(&path, mode));
        cx.notify();
    }

    /// 行テキスト全体を 1 ランで shape するための TextRun（幅計算用。色は使われない）
    fn mono_text_run(&self, len: usize) -> TextRun {
        TextRun {
            len,
            font: gpui::font(self.theme.font_family.clone()),
            color: hsla(self.theme.foreground),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    fn on_pane_mouse_down(
        &mut self,
        pane_id: PaneId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.workspace.active_tab_mut().tree_mut().focus(pane_id);
        if let Some((col, row, right)) = self.cell_at(pane_id, event.position, window) {
            if let Some(session) = self.terminals.get(&pane_id) {
                let kind = match event.click_count {
                    1 => SelectionKind::Simple,
                    2 => SelectionKind::Word,
                    _ => SelectionKind::Line,
                };
                session.clear_selection();
                session.start_selection(kind, col, row, right);
                self.selecting = Some(pane_id);
            }
        }
        cx.notify();
    }

    /// 境界ハンドルの押下でドラッグ開始（リサイズ）。選択は始めない
    fn start_border_drag(&mut self, border: DragBorder, cx: &mut Context<Self>) {
        self.dragging_border = Some(border);
        cx.notify();
    }

    /// スクロールバーの押下: その位置へジャンプし、ドラッグ追従を開始する
    fn start_scrollbar_drag(&mut self, pane_id: PaneId, y: Pixels, cx: &mut Context<Self>) {
        self.dragging_scrollbar = Some(pane_id);
        self.scrollbar_drag_to(pane_id, y, cx);
    }

    /// マウス y 座標をスクロールバック位置へ換算する（サム中心 = マウス位置）
    fn scrollbar_drag_to(&mut self, pane_id: PaneId, y: Pixels, cx: &mut Context<Self>) {
        let Some((_, area)) = self.pane_text_areas.iter().find(|(id, _)| *id == pane_id) else {
            return;
        };
        let area = *area;
        let backend = self.backend_sessions.contains_key(&pane_id);
        let Some(session) = self.terminals.get(&pane_id) else {
            return;
        };
        let (_, rows) = session.size();
        let history = if backend {
            self.scroll_ctls
                .get(&pane_id)
                .map(|c| c.state.history)
                .unwrap_or(0)
        } else {
            session.history_size()
        };
        let total = (history + rows) as f32;
        let ratio = ((f32::from(y) - f32::from(area.origin.y)) / f32::from(area.size.height))
            .clamp(0.0, 1.0);
        // 表示窓（rows 行）の中心をマウス位置の行へ合わせ、上端行 → offset に直す
        let top_row = (ratio * total - rows as f32 / 2.0).clamp(0.0, history as f32);
        let offset = history - top_row.round() as usize;
        if backend {
            let ctl = self.scroll_ctls.entry(pane_id).or_default();
            ctl.last_activity = std::time::Instant::now();
            ctl.drag_goal = Some(offset);
            ctl.pending = 0;
            self.pump_scroll(pane_id, cx);
            self.ensure_scroll_ticker(cx);
        } else {
            session.scroll_to(offset);
            self.mark_scroll_activity(pane_id, cx);
        }
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) {
            // ウィンドウ外でボタンが離されると MouseUp が届かないことがある。
            // 取り残したドラッグ・選択状態はここで畳む（残留すると以後どこを
            // 左ドラッグしてもリサイズが発火し「当たり判定が広がった」ように見える）
            if self.dragging_border.take().is_some()
                | self.dragging_scrollbar.take().is_some()
                | self.selecting.take().is_some()
                | std::mem::take(&mut self.dragging_panel)
            {
                cx.notify();
            }
            return;
        }
        // 情報パネルの幅ドラッグ
        if self.dragging_panel {
            let total = f32::from(window.viewport_size().width);
            let max = (total * 0.7).max(PANEL_MIN_WIDTH);
            self.panel_width = (total - f32::from(event.position.x)).clamp(PANEL_MIN_WIDTH, max);
            cx.notify();
            return;
        }
        // スクロールバードラッグ中はスクロール位置を追従させる
        if let Some(pane_id) = self.dragging_scrollbar {
            self.scrollbar_drag_to(pane_id, event.position.y, cx);
            return;
        }
        // 境界ドラッグ中は分割比率を更新（PTY リサイズは次の render の追従に任せる）
        if let Some(drag) = self.dragging_border {
            let ratio = ratio_for_position(
                drag.area,
                drag.axis,
                f32::from(event.position.x),
                f32::from(event.position.y),
            );
            self.workspace
                .active_tab_mut()
                .tree_mut()
                .set_split_ratio(drag.index, ratio);
            cx.notify();
            return;
        }
        let Some(pane_id) = self.selecting else {
            return;
        };
        if let Some((col, row, right)) = self.cell_at(pane_id, event.position, window) {
            if let Some(session) = self.terminals.get(&pane_id) {
                session.extend_selection(col, row, right);
                cx.notify();
            }
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.dragging_border.take().is_some()
            | self.dragging_scrollbar.take().is_some()
            | std::mem::take(&mut self.dragging_panel)
        {
            cx.notify();
            return;
        }
        if let Some(pane_id) = self.selecting.take() {
            // iTerm2 流の copy-on-select
            if let Some(text) = self
                .terminals
                .get(&pane_id)
                .and_then(|s| s.selection_text())
            {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            cx.notify();
        }
    }

    fn on_pane_scroll(
        &mut self,
        pane_id: PaneId,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cell) = self.cell_size else {
            return;
        };
        let delta_lines = match event.delta {
            ScrollDelta::Lines(l) => l.y * 3.0,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(cell.height),
        };
        // トラックパッドはイベント単位のピクセルデルタが 1 セル未満になりがちで、
        // 都度切り捨てるとゆっくりスクロールが完全に無反応になる（2026-06-12
        // 実機バグ (1) の app 側要因）。端数をペインごとに持ち越して積分する
        let carry = self.scroll_accum.entry(pane_id).or_insert(0.0);
        let (lines, rest) = accumulate_scroll(*carry, delta_lines);
        *carry = rest;
        if lines != 0 {
            let (col, row) = self
                .cell_at(pane_id, event.position, window)
                .map(|(c, r, _)| (c, r))
                .unwrap_or((0, 0));
            if self.backend_sessions.contains_key(&pane_id) {
                // バックエンドペイン: tako が tmux スクロールを正確な行数で駆動する
                self.backend_scroll(pane_id, lines, (col, row), cx);
            } else if let Some(session) = self.terminals.get(&pane_id) {
                // 直接ペイン: mouse reporting / alternate scroll / 自前スクロールの
                // 出し分けはセッション側
                session.scroll_wheel(lines, col, row);
                self.mark_scroll_activity(pane_id, cx);
                cx.notify();
            }
        }
    }

    // --- バックエンド / ネスト tmux スクロール（tako-core::scroll の UI 側） ---

    /// ホイール行数をバックエンドスクロールへ積む。マウス要求アプリ（vim 等）へは
    /// 従来どおり生 SGR を転送し、それ以外は tako 自身が tmux copy-mode を正確な
    /// 行数で駆動する。SGR 経由で tmux 既定バインドの copy-mode に入れる方式は
    /// 「5 行単位でばっと飛ぶ」「キー入力が copy-mode に飲まれる」「copy-mode
    /// カーソルが画面に居座る」の 3 症状を生むためやめた（2026-06-12 実機）
    fn backend_scroll(
        &mut self,
        pane_id: PaneId,
        lines: i32,
        cell: (usize, usize),
        cx: &mut Context<Self>,
    ) {
        let ctl = self.scroll_ctls.entry(pane_id).or_default();
        ctl.last_activity = std::time::Instant::now();
        ctl.last_cell = cell;
        if ctl.target.is_some() && ctl.state.wants_mouse {
            if let Some(session) = self.terminals.get(&pane_id) {
                session.scroll_wheel(lines, cell.0, cell.1);
            }
        } else {
            ctl.pending += lines;
            self.pump_scroll(pane_id, cx);
        }
        self.ensure_scroll_ticker(cx);
        cx.notify();
    }

    /// 溜まったスクロール要求を 1 つの tmux 操作として実行する（ペイン単位に直列 =
    /// コアレッシング）。完了時に残りがあれば再帰的にポンプする
    fn pump_scroll(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(backend) = self.backend_sessions.get(&pane_id).cloned() else {
            return;
        };
        let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) else {
            return;
        };
        if ctl.in_flight {
            return;
        }
        let need_resolve = ctl.target.is_none();
        let goal = ctl.drag_goal.take();
        let delta = std::mem::take(&mut ctl.pending);
        let refresh = std::mem::take(&mut ctl.want_refresh);
        if !need_resolve && goal.is_none() && delta == 0 && !refresh {
            return;
        }
        ctl.in_flight = true;
        ctl.last_refresh = std::time::Instant::now();
        let target = ctl.target.clone();
        let socket = tako_core::tmux_backend::socket_name();
        cx.spawn(async move |this, cx| {
            let task = cx.background_executor().spawn(async move {
                use tako_core::scroll;
                let target =
                    target.unwrap_or_else(|| scroll::resolve_target(&socket, &backend, &[None]));
                let state = if let Some(goal) = goal {
                    scroll::scroll_to(&target, goal)
                } else if delta != 0 {
                    scroll::scroll_by(&target, delta)
                } else {
                    scroll::scroll_state(&target)
                };
                (target, state)
            });
            let (target, state) = task.await;
            this.update(cx, |app, cx| {
                let mut flush: Option<i32> = None;
                if let Some(ctl) = app.scroll_ctls.get_mut(&pane_id) {
                    ctl.in_flight = false;
                    ctl.target = Some(target);
                    if let Some(state) = state {
                        ctl.state = state;
                        // 解決して初めてマウス要求アプリと判明したら、待つ間に
                        // 溜まった分を生 SGR へ振り替える
                        if state.wants_mouse && ctl.pending != 0 {
                            flush = Some(std::mem::take(&mut ctl.pending));
                        }
                    }
                }
                if let Some(lines) = flush {
                    let cell = app
                        .scroll_ctls
                        .get(&pane_id)
                        .map(|c| c.last_cell)
                        .unwrap_or((0, 0));
                    if let Some(session) = app.terminals.get(&pane_id) {
                        session.scroll_wheel(lines, cell.0, cell.1);
                    }
                } else {
                    app.pump_scroll(pane_id, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// dispatch（CLI / MCP）の Scroll 応答を UI のスクロール状態へ反映する
    fn sync_scroll_from_dispatch(&mut self, value: &serde_json::Value, cx: &mut Context<Self>) {
        let (Some(pane), Some(offset), Some(history)) = (
            value["pane"].as_u64(),
            value["offset"].as_u64(),
            value["history"].as_u64(),
        ) else {
            return;
        };
        let Some(pane_id) = self
            .workspace
            .tabs()
            .iter()
            .flat_map(|t| t.tree().panes())
            .map(|p| p.id())
            .find(|id| id.as_u64() == pane)
        else {
            return;
        };
        if self.backend_sessions.contains_key(&pane_id) {
            let ctl = self.scroll_ctls.entry(pane_id).or_default();
            ctl.last_activity = std::time::Instant::now();
            ctl.state.position = offset as usize;
            ctl.state.history = history as usize;
            ctl.state.in_mode = offset > 0;
            // dispatch 側で解決済みだが UI 側の target は未解決のままにし、
            // 次のホイール / キー時に必要なら解決する（cancel は target が要るため）
            if offset > 0 && ctl.target.is_none() {
                ctl.want_refresh = true;
                self.pump_scroll(pane_id, cx);
            }
            self.ensure_scroll_ticker(cx);
        } else {
            self.mark_scroll_activity(pane_id, cx);
        }
        cx.notify();
    }

    /// 直接ペインのスクロール活動を記録する（スクロールバーのフェード表示トリガー）
    fn mark_scroll_activity(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        self.scroll_ctls.entry(pane_id).or_default().last_activity = std::time::Instant::now();
        self.ensure_scroll_ticker(cx);
    }

    /// copy-mode 中のキー入力前に最下部へ戻す（iTerm2 流）。同期実行（~数 ms）なのは
    /// 非同期にするとキーが先に copy-mode へ届いて飲まれるため
    fn cancel_scroll_before_input(&mut self, pane_id: PaneId) {
        if let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) {
            if ctl.state.in_mode {
                if let Some(target) = &ctl.target {
                    tako_core::scroll::cancel(target);
                }
                ctl.state.in_mode = false;
                ctl.state.position = 0;
                ctl.pending = 0;
                ctl.drag_goal = None;
            }
        }
    }

    /// スクロールバーのフェード再描画と copy-mode 状態の追従。
    /// 対象エントリが尽きたら自動停止する
    fn ensure_scroll_ticker(&mut self, cx: &mut Context<Self>) {
        if self.scroll_ticker {
            return;
        }
        self.scroll_ticker = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let live = this
                    .update(cx, |app, cx| {
                        let dragging = app.dragging_scrollbar;
                        app.scroll_ctls.retain(|id, ctl| {
                            ctl.in_flight
                                || ctl.pending != 0
                                || ctl.drag_goal.is_some()
                                || ctl.state.in_mode
                                || Some(*id) == dragging
                                || scrollbar_alpha(ctl.last_activity.elapsed().as_millis()) > 0.0
                        });
                        // copy-mode 中は外部変化（ユーザーの q・新規出力での履歴増）に追従
                        let refresh: Vec<PaneId> = app
                            .scroll_ctls
                            .iter_mut()
                            .filter(|(_, ctl)| {
                                ctl.state.in_mode
                                    && !ctl.in_flight
                                    && ctl.last_refresh.elapsed() >= Duration::from_millis(1000)
                            })
                            .map(|(id, ctl)| {
                                ctl.want_refresh = true;
                                *id
                            })
                            .collect();
                        for id in refresh {
                            app.pump_scroll(id, cx);
                        }
                        cx.notify();
                        !app.scroll_ctls.is_empty()
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
            }
            this.update(cx, |app, _| app.scroll_ticker = false).ok();
        })
        .detach();
    }

    /// スクロールバーの描画情報 (top, thumb_h, track_h, alpha, dragging)。
    /// スクロール活動が無い・フェードアウト済み・履歴ゼロでは None（iTerm2 流）
    fn scrollbar_overlay(
        &self,
        pane_id: PaneId,
        area: Bounds<Pixels>,
    ) -> Option<(f32, f32, f32, f32, bool)> {
        let dragging = self.dragging_scrollbar == Some(pane_id);
        let ctl = self.scroll_ctls.get(&pane_id)?;
        let alpha = if dragging {
            1.0
        } else {
            scrollbar_alpha(ctl.last_activity.elapsed().as_millis())
        };
        if alpha <= 0.0 {
            return None;
        }
        let session = self.terminals.get(&pane_id)?;
        let (offset, history) = if self.backend_sessions.contains_key(&pane_id) {
            // バックエンドのスクロールバックは tmux 側（ネスト先含む）にある
            (ctl.state.position, ctl.state.history)
        } else {
            if session.is_alt_screen() {
                return None;
            }
            (session.display_offset(), session.history_size())
        };
        if history == 0 {
            return None;
        }
        let (_, rows) = session.size();
        let total = (history + rows) as f32;
        let track_h = f32::from(area.size.height);
        let thumb_h = (rows as f32 / total * track_h).clamp(20.0, track_h);
        let top = ((history - offset.min(history)) as f32 / total * track_h).min(track_h - thumb_h);
        Some((top, thumb_h, track_h, alpha, dragging))
    }

    // --- 描画 ---

    /// セル寸法を実測する（等幅前提で 'M' の advance + テーマ行高）
    fn measure_cell(&mut self, window: &mut Window) -> Size<Pixels> {
        if let Some(cell) = self.cell_size {
            return cell;
        }
        let font = Font {
            family: SharedString::from(self.theme.font_family.clone()),
            ..gpui::font(self.theme.font_family.clone())
        };
        let font_id = window.text_system().resolve_font(&font);
        let width = window
            .text_system()
            .advance(font_id, px(self.theme.font_size), 'M')
            .map(|advance| advance.width)
            // 計測に失敗してもセル幅ゼロで詰まないよう概算へフォールバック
            .unwrap_or(px(self.theme.font_size * 0.6));
        let cell = size(width, px(self.theme.line_height));
        self.cell_size = Some(cell);
        cell
    }

    fn text_style(&self) -> TextStyle {
        TextStyle {
            color: hsla(self.theme.foreground),
            font_family: SharedString::from(self.theme.font_family.clone()),
            font_size: px(self.theme.font_size).into(),
            line_height: px(self.theme.line_height).into(),
            ..TextStyle::default()
        }
    }

    fn render_tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.workspace.active_tab_id();
        let tabs: Vec<_> = self
            .workspace
            .tabs()
            .iter()
            .map(|tab| {
                let id = tab.id();
                // タブ表示名: リネーム済み（自動 FR-2.12 / 手動）はタブ名を、
                // 未設定（連番のまま）はフォーカス中ペインの OSC タイトルを優先する
                let label = if tab.title_source() == TitleSource::Default {
                    tab.tree()
                        .panes()
                        .iter()
                        .find(|p| p.id() == tab.tree().focused())
                        .and_then(|p| self.terminals.get(&p.id()))
                        .and_then(|s| s.title())
                        .unwrap_or(tab.title())
                        .to_string()
                } else {
                    tab.title().to_string()
                };
                // タブ内ペイン状態の集約ドット（FR-2.1.4）: エラー > 実行中のみ表示
                let dot = match CommandState::aggregate(
                    tab.tree()
                        .panes()
                        .iter()
                        .filter_map(|p| self.terminals.get(&p.id()))
                        .map(|s| s.command_state()),
                ) {
                    CommandState::Failed(_) => Some(theme.ansi[1]), // 赤
                    CommandState::Running => Some(theme.accent),
                    _ => None,
                };
                (id, label, dot)
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            // ルート flex 列が窮屈になっても高さを譲らない（ステータスバー消失と同根の予防）
            .flex_none()
            .w_full()
            .bg(rgba(theme.tab_bar_background))
            .children(tabs.into_iter().map(|(id, label, dot)| {
                let is_active = id == active;
                div()
                    .id(("tab", id.as_u64()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h_full()
                    .px_3()
                    .cursor_pointer()
                    .when(is_active, |d| {
                        d.bg(rgba(theme.tab_active_background))
                            .border_b_2()
                            .border_color(hsla(theme.accent))
                    })
                    .text_color(if is_active {
                        hsla(theme.tab_active_foreground)
                    } else {
                        hsla(theme.tab_inactive_foreground)
                    })
                    .text_size(px(12.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let _ = this.workspace.activate_tab(id);
                        cx.notify();
                    }))
                    .children(
                        dot.map(|color| div().w(px(6.0)).h(px(6.0)).rounded_full().bg(hsla(color))),
                    )
                    .child(SharedString::from(truncate(&label, 24)))
                    .child(
                        div()
                            .id(("tab-close", id.as_u64()))
                            .px_1()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_tab(id, cx);
                            }))
                            .child("×"),
                    )
            }))
            .child(
                div()
                    .id("tab-new")
                    .px_3()
                    .cursor_pointer()
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .on_click(cx.listener(|this, _, _, cx| this.new_tab(cx)))
                    .child("+"),
            )
            .child(div().flex_grow(1.0))
        // 旧「◧ panel」トグルは下部ステータスバーへ集約済み（FR-2.16.4）
    }

    /// 下部ステータスバー（FR-2.16.4。Zed / VSCode 風）。
    /// 左 = ファイルツリートグル、右 = tmux 管理・git 管理トグル。
    /// トグル状態は dispatch（`tako panel` / MCP `tako_panel`）からも取得・操作できる
    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme.clone();
        // 全ペイン集約の状態ドット（旧 agents 固定タブ → ◧ panel から引き継ぎ。FR-2.10）
        let agents_dot =
            match CommandState::aggregate(self.terminals.values().map(|s| s.command_state())) {
                CommandState::Failed(_) => Some(theme.ansi[1]),
                CommandState::Running => Some(theme.accent),
                _ => None,
            };
        let toggle = |id: &'static str, active: bool| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h_full()
                .px_2()
                .cursor_pointer()
                .text_size(px(11.0))
                .when(active, |d| d.bg(rgba(theme.tab_active_background)))
                .text_color(if active {
                    hsla(theme.tab_active_foreground)
                } else {
                    hsla(theme.tab_inactive_foreground)
                })
                .hover(|d| d.bg(rgba_alpha(theme.tab_active_background, 0.7)))
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            // ルート flex 列が窮屈になっても高さを譲らない（消失バグの再発防止。
            // 根本対策は中段の min_h(0) — 下の render 末尾コメント参照）
            .flex_none()
            .w_full()
            .bg(rgba(theme.tab_bar_background))
            .border_t_1()
            .border_color(hsla(theme.pane_border))
            .child(
                toggle("statusbar-filetree", self.filetree.visible)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_filetree();
                        cx.notify();
                    }))
                    .child("◫ ファイル"),
            )
            .child(div().flex_grow(1.0))
            .child(
                toggle(
                    "statusbar-tmux",
                    self.panel_visible && self.panel_view == PanelView::Tmux,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_panel_view(PanelView::Tmux, cx);
                }))
                .children(
                    agents_dot
                        .map(|color| div().w(px(6.0)).h(px(6.0)).rounded_full().bg(hsla(color))),
                )
                .child("⌗ tmux"),
            )
            .child(
                toggle(
                    "statusbar-git",
                    self.panel_visible && self.panel_view == PanelView::Git,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_panel_view(PanelView::Git, cx);
                }))
                .child("⎇ git"),
            )
    }

    /// ファイルツリーの表示トグル（cmd+B / ステータスバー / dispatch 共通の入口）
    fn toggle_filetree(&mut self) {
        self.filetree.visible = !self.filetree.visible;
        // render を待たず即座に root を同期する（オクルージョン中は GPUI が
        // 再描画しないため、render 内の追従だけだと開いた直後に空になる）
        self.sync_filetree_roots();
    }

    /// ステータスバーの tmux / git トグル: 同じビューが開いていれば閉じ、
    /// 違うビューならそのビューへ切り替えて開く（FR-2.16.4）
    fn toggle_panel_view(&mut self, view: PanelView, cx: &mut Context<Self>) {
        if self.panel_visible && self.panel_view == view {
            self.panel_visible = false;
        } else {
            self.panel_visible = true;
            self.panel_view = view;
            if view == PanelView::Tmux {
                self.refresh_tmux(cx);
            }
        }
        cx.notify();
    }

    /// 右サイドバー情報パネル（非表示なら None）。内部タブは統合 tmux ビュー
    /// （FR-2.16.6）と git（git graph FR-3.6 実装まではプレースホルダ）の 2 本
    fn render_panel(&mut self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.panel_visible {
            return None;
        }
        let theme = self.theme.clone();
        let view = self.panel_view;
        let tab_button = |label: &'static str, target: PanelView, active: bool| {
            div()
                .id(("panel-tab", target as u64))
                .px_2()
                .py_1()
                .cursor_pointer()
                .text_size(px(11.0))
                .when(active, |d| {
                    d.bg(rgba(theme.tab_active_background))
                        .border_b_2()
                        .border_color(hsla(theme.accent))
                })
                .text_color(if active {
                    hsla(theme.tab_active_foreground)
                } else {
                    hsla(theme.tab_inactive_foreground)
                })
                .child(label)
        };
        Some(
            div()
                .w(px(self.panel_width))
                .h_full()
                .relative()
                .flex()
                .flex_col()
                .bg(rgba(theme.background))
                .border_l_1()
                .border_color(hsla(theme.pane_border))
                .overflow_hidden()
                .child(
                    // 内部タブヘッダ（切り替え式コンテナ。右端 × でパネルを閉じる）
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .h(px(26.0))
                        .flex_none()
                        .bg(rgba(theme.tab_bar_background))
                        .child(
                            tab_button("tmux", PanelView::Tmux, view == PanelView::Tmux).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.panel_view = PanelView::Tmux;
                                    this.refresh_tmux(cx);
                                }),
                            ),
                        )
                        .child(
                            tab_button("git", PanelView::Git, view == PanelView::Git).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.panel_view = PanelView::Git;
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(div().flex_grow(1.0))
                        .child(
                            div()
                                .id("panel-close")
                                .px_1()
                                .cursor_pointer()
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.panel_visible = false;
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                )
                .child(match view {
                    PanelView::Tmux => self.render_tmux_view(cx).into_any_element(),
                    PanelView::Git => self.render_git_view().into_any_element(),
                })
                .child(
                    // 左端のリサイズハンドル（ドラッグで幅調整）
                    div()
                        .id("panel-resize")
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(BORDER_HANDLE))
                        .h_full()
                        .cursor(CursorStyle::ResizeLeftRight)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.dragging_panel = true;
                                cx.stop_propagation();
                            }),
                        ),
                ),
        )
    }

    fn render_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: Bounds<Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // プレビューペイン（FR-3.2 / FR-3.3）はターミナルではなくファイル内容を描く
        if self.previews.contains_key(&pane_id) {
            return self
                .render_preview_pane(pane_id, rect, focused, cx)
                .into_any_element();
        }
        let theme = self.theme.clone();
        let default_style = self.text_style();
        let cell = self.cell_size.expect("render 冒頭で実測済み");

        // PTY リサイズ追従: テキスト領域に収まる cols/rows へ
        let cols = (f32::from(area.size.width) / f32::from(cell.width)).floor() as usize;
        let rows = (f32::from(area.size.height) / f32::from(cell.height)).floor() as usize;
        if let Some(session) = self.terminals.get_mut(&pane_id) {
            session.resize(
                cols,
                rows,
                f32::from(cell.width).round() as u16,
                f32::from(cell.height).round() as u16,
            );
        }

        // タイトルバーの表示名（FR-2.1.3。iTerm2 風: 手動 / AI リネーム > role > OSC タイトル）。
        // コマンド実行状態（FR-2.1.4）はドット色で添える: 赤 = エラー、アクセント = 実行中
        let badge_label = self
            .workspace
            .active_tab()
            .tree()
            .get(pane_id)
            .and_then(|p| match (p.title(), p.role()) {
                (Some(t), Some(r)) => Some(format!("{t} · {r}")),
                (Some(t), None) => Some(t.to_string()),
                (None, Some(r)) => Some(r.to_string()),
                (None, None) => None,
            });
        let title_label = badge_label
            .or_else(|| {
                self.terminals
                    .get(&pane_id)
                    .and_then(|s| s.title())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "ターミナル".to_string());
        let state_dot = self
            .terminals
            .get(&pane_id)
            .and_then(|s| match s.command_state() {
                tako_core::CommandState::Failed(_) => Some(theme.ansi[1]),
                tako_core::CommandState::Running => Some(theme.accent),
                _ => None,
            });

        // スクロールバー（FR-2.5.13）: iTerm2 流にスクロール中だけ表示 → フェードアウト。
        // バックエンドペインは tmux 側（ネスト先含む）の位置・履歴を表示する
        let scrollbar = self.scrollbar_overlay(pane_id, area);

        // 提案チップ（FR-2.4.3）。このペインの先頭 1 件だけ下端に出す（残りは閉じたら順に）
        let suggestion = self
            .port_suggestions
            .iter()
            .find(|s| s.pane == pane_id)
            .map(|s| (s.port, s.process.clone()));

        // tmux copy-mode でスクロール中は copy-mode カーソルが画面に固定表示されて
        // 不自然なため隠す（2026-06-12 実機フィードバック (b)）
        let scrolled_in_tmux = self
            .scroll_ctls
            .get(&pane_id)
            .is_some_and(|c| c.state.in_mode);
        let screen = self
            .terminals
            .get(&pane_id)
            .map(|s| s.screen_opts(&theme, !scrolled_in_tmux));

        let lines: Vec<_> = screen
            .map(|screen| {
                screen
                    .lines
                    .into_iter()
                    .map(|line| {
                        let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = line
                            .runs
                            .iter()
                            .map(|run| {
                                (
                                    run.range.clone(),
                                    HighlightStyle {
                                        color: Some(hsla(run.fg)),
                                        background_color: run.bg.map(hsla),
                                        font_weight: run.bold.then_some(FontWeight::BOLD),
                                        font_style: run.italic.then_some(FontStyle::Italic),
                                        underline: run.underline.then_some(UnderlineStyle {
                                            thickness: px(1.0),
                                            color: None,
                                            wavy: false,
                                        }),
                                        strikethrough: run.strikeout.then_some(
                                            StrikethroughStyle {
                                                thickness: px(1.0),
                                                color: None,
                                            },
                                        ),
                                        fade_out: None,
                                    },
                                )
                            })
                            .collect();
                        div().h(px(theme.line_height)).child(
                            StyledText::new(line.text)
                                .with_default_highlights(&default_style, highlights),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        div()
            .id(("pane", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .bg(rgba(theme.background))
            .border(px(PANE_BORDER))
            .border_color(if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.pane_border)
            })
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.on_pane_mouse_down(pane_id, event, window, cx);
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &ScrollWheelEvent, window, cx| {
                    this.on_pane_scroll(pane_id, event, window, cx);
                }),
            )
            .child(
                // ペイン上部のタイトルバー（iTerm2 風。FR-2.1.3 のバッジを置き換える主表示）:
                // 左に分かりやすい × ボタン、状態ドット、ペイン名（手動 / AI リネーム）
                div()
                    .id(("pane-titlebar", pane_id.as_u64()))
                    .h(px(PANE_TITLE_BAR))
                    .flex_none()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .bg(rgba_alpha(
                        theme.tab_bar_background,
                        if focused { 1.0 } else { 0.6 },
                    ))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .on_mouse_down(
                        MouseButton::Left,
                        // タイトルバーからは選択を開始しない（フォーカスだけ移す）
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.workspace
                                .active_tab_mut()
                                .tree_mut()
                                .focus(pane_id)
                                .ok();
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id(("pane-close", pane_id.as_u64()))
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                            .hover(|d| {
                                d.bg(rgba_alpha(theme.ansi[1], 0.25))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_pane_button(pane_id, cx);
                            }))
                            .child("×"),
                    )
                    .children(
                        state_dot.map(|color| {
                            div().w(px(6.0)).h(px(6.0)).rounded_full().bg(hsla(color))
                        }),
                    )
                    .child(
                        div()
                            .text_color(if focused {
                                hsla(theme.foreground)
                            } else {
                                hsla(theme.tab_inactive_foreground)
                            })
                            .child(SharedString::from(truncate(&title_label, 48))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .p(px(PANE_PADDING))
                    .overflow_hidden()
                    .children(lines),
            )
            .children(scrollbar.map(|(top, thumb_h, track_h, alpha, dragging)| {
                div()
                    .id(("scrollbar", pane_id.as_u64()))
                    .absolute()
                    .top(px(PANE_TITLE_BAR))
                    .right(px(0.0))
                    .w(px(SCROLLBAR_WIDTH))
                    .h(px(track_h))
                    .occlude() // 下のペインへの選択開始を防ぐ
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.start_scrollbar_drag(pane_id, event.position.y, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(top))
                            .right(px(2.0))
                            .w(px(SCROLLBAR_WIDTH - 4.0))
                            .h(px(thumb_h))
                            .rounded_sm()
                            .bg(rgba_alpha(
                                theme.tab_inactive_foreground,
                                alpha * if dragging { 0.7 } else { 0.35 },
                            )),
                    )
            }))
            .children(suggestion.map(|(port, process)| {
                // 提案チップ（FR-2.4.3）: 検知ペイン下端のインライン表示。
                // 承諾アクションは open_preview（当面は外部ブラウザ。差し替え点）
                let label = if process.is_empty() {
                    format!("localhost:{port} が listen 中")
                } else {
                    format!("localhost:{port}（{process}）が listen 中")
                };
                div()
                    .id(("port-chip", pane_id.as_u64()))
                    .absolute()
                    .bottom(px(4.0))
                    .left(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .occlude() // 下のペインへの選択開始を防ぐ
                    .bg(rgba(theme.tab_bar_background))
                    .border_1()
                    .border_color(hsla(theme.accent))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.foreground))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                    )
                    .child(SharedString::from(label))
                    .child(
                        div()
                            .id(("port-chip-open", pane_id.as_u64()))
                            .cursor_pointer()
                            .text_color(hsla(theme.accent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.accept_port_suggestion(pane_id, port, cx);
                            }))
                            .child("ブラウザで開く"),
                    )
                    .child(
                        div()
                            .id(("port-chip-dismiss", pane_id.as_u64()))
                            .cursor_pointer()
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.7))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.dismiss_port_suggestion(pane_id, port, cx);
                            }))
                            .child("×"),
                    )
            }))
            .into_any_element()
    }

    /// プレビューペインの描画（FR-3.2 コード / FR-3.3 Markdown）。
    /// タイトルバーはファイル名 + （.md のみ）目アイコンのモードトグル + × ボタン
    fn render_preview_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let state = self.previews.get(&pane_id).expect("呼び出し前に確認済み");
        let file_name = state.file_name();
        let path_label = state.path.display().to_string();
        let md_capable = state.markdown_capable();
        let mode = state.mode;
        let truncated = state.truncated;

        // 本文要素を先に組む（state の借用をここで終える）
        let body: Vec<gpui::AnyElement> = match &state.content {
            preview::PreviewContent::Code(lines) => {
                let number_width = lines.len().to_string().len();
                lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| {
                        self.preview_code_line(line, Some((i + 1, number_width)))
                            .into_any_element()
                    })
                    .collect()
            }
            preview::PreviewContent::Markdown(blocks) => blocks
                .iter()
                .map(|block| self.preview_md_block(block))
                .collect(),
            preview::PreviewContent::Error(message) => vec![div()
                .p_2()
                .text_color(hsla(theme.ansi[1]))
                .child(SharedString::from(message.clone()))
                .into_any_element()],
        };

        div()
            .id(("pane", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .bg(rgba(theme.background))
            .border(px(PANE_BORDER))
            .border_color(if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.pane_border)
            })
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    let _ = this.workspace.active_tab_mut().tree_mut().focus(pane_id);
                    cx.notify();
                }),
            )
            .child(
                // タイトルバー: × / 📄 ファイル名 / （md のみ）モードトグル
                div()
                    .id(("preview-titlebar", pane_id.as_u64()))
                    .h(px(PANE_TITLE_BAR))
                    .flex_none()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .bg(rgba_alpha(
                        theme.tab_bar_background,
                        if focused { 1.0 } else { 0.6 },
                    ))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .child(
                        div()
                            .id(("pane-close", pane_id.as_u64()))
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                            .hover(|d| {
                                d.bg(rgba_alpha(theme.ansi[1], 0.25))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_pane_button(pane_id, cx);
                            }))
                            .child("×"),
                    )
                    .child(
                        div()
                            .text_color(if focused {
                                hsla(theme.foreground)
                            } else {
                                hsla(theme.tab_inactive_foreground)
                            })
                            .child(SharedString::from(format!(
                                "📄 {}",
                                truncate(&file_name, 36)
                            ))),
                    )
                    .child(div().flex_grow(1.0))
                    .children(md_capable.then(|| {
                        // 目アイコンのトグル（FR-3.3）: コード表示 ⇔ md レンダリング
                        let (icon, label) = match mode {
                            preview::PreviewMode::Markdown => ("</>", "コードとして表示"),
                            preview::PreviewMode::Code => ("👁", "md レンダリング表示"),
                        };
                        div()
                            .id(("preview-mode-toggle", pane_id.as_u64()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hsla(theme.accent))
                            .hover(|d| d.bg(rgba_alpha(theme.tab_active_background, 0.8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_preview_mode(pane_id, cx);
                            }))
                            .child(SharedString::from(format!("{icon} {label}")))
                    }))
                    .child(
                        div()
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                            .text_size(px(10.0))
                            .child(SharedString::from(truncate(&path_label, 40))),
                    ),
            )
            .child(
                div()
                    .id(("preview-scroll", pane_id.as_u64()))
                    .flex_1()
                    .p(px(PANE_PADDING + 4.0))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .children(body)
                    .children(truncated.then(|| {
                        div()
                            .pt_2()
                            .text_size(px(11.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                            .child("…（大きいファイルのため末尾を省略して表示）")
                    })),
            )
    }

    /// ハイライト済みコード 1 行 → StyledText（行番号は控えめな色で前置する）
    fn preview_code_line(&self, line: &preview::Line, number: Option<(usize, usize)>) -> gpui::Div {
        let theme = &self.theme;
        let mut text = String::new();
        let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
        if let Some((n, width)) = number {
            let prefix = format!("{n:>width$}  ");
            highlights.push((
                0..prefix.len(),
                HighlightStyle {
                    color: Some(hsla_alpha(theme.tab_inactive_foreground, 0.5)),
                    ..HighlightStyle::default()
                },
            ));
            text.push_str(&prefix);
        }
        for span in line {
            let start = text.len();
            text.push_str(&span.text);
            let style = HighlightStyle {
                color: span.color.map(hsla),
                font_weight: span.bold.then_some(FontWeight::BOLD),
                font_style: span.italic.then_some(FontStyle::Italic),
                ..HighlightStyle::default()
            };
            if span.color.is_some() || span.bold || span.italic {
                highlights.push((start..text.len(), style));
            }
        }
        if text.is_empty() {
            // 空行も高さを保つ
            text.push(' ');
        }
        div()
            .h(px(theme.line_height))
            .flex_none()
            .child(StyledText::new(text).with_default_highlights(&self.text_style(), highlights))
    }

    /// Markdown インラインスパン列 → (テキスト, ハイライト範囲)
    fn preview_md_text(
        &self,
        spans: &[preview::MdSpan],
    ) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
        let theme = &self.theme;
        let mut text = String::new();
        let mut highlights = Vec::new();
        for span in spans {
            let start = text.len();
            text.push_str(&span.text);
            let styled = span.bold || span.italic || span.code || span.strike || span.link;
            if !styled {
                continue;
            }
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: if span.code {
                        Some(hsla(theme.ansi[3]))
                    } else if span.link {
                        Some(hsla(theme.accent))
                    } else {
                        None
                    },
                    background_color: span.code.then(|| hsla(theme.tab_bar_background)),
                    font_weight: span.bold.then_some(FontWeight::BOLD),
                    font_style: span.italic.then_some(FontStyle::Italic),
                    underline: span.link.then(|| UnderlineStyle {
                        thickness: px(1.0),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: span.strike.then_some(StrikethroughStyle {
                        thickness: px(1.0),
                        color: None,
                    }),
                    ..HighlightStyle::default()
                },
            ));
        }
        (text, highlights)
    }

    /// Markdown ブロック 1 つの描画（FR-3.3）
    fn preview_md_block(&self, block: &preview::MdBlock) -> gpui::AnyElement {
        let theme = self.theme.clone();
        match block {
            preview::MdBlock::Heading { level, spans } => {
                let (text, highlights) = self.preview_md_text(spans);
                let size = match level {
                    1 => 19.0,
                    2 => 16.0,
                    3 => 14.0,
                    _ => 13.0,
                };
                div()
                    .pt_2()
                    .pb_1()
                    .text_size(px(size))
                    .font_weight(FontWeight::BOLD)
                    .text_color(hsla(theme.foreground))
                    .when(*level <= 2, |d| {
                        d.border_b_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.8))
                    })
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .into_any_element()
            }
            preview::MdBlock::Paragraph { spans } => {
                let (text, highlights) = self.preview_md_text(spans);
                div()
                    .py_1()
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .into_any_element()
            }
            preview::MdBlock::ListItem {
                depth,
                marker,
                spans,
            } => {
                let (text, highlights) = self.preview_md_text(spans);
                div()
                    .flex()
                    .flex_row()
                    .pl(px(8.0 + 16.0 * *depth as f32))
                    .gap_1()
                    .child(
                        div()
                            .flex_none()
                            .text_color(hsla_alpha(theme.foreground, 0.7))
                            .child(SharedString::from(marker.clone())),
                    )
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .into_any_element()
            }
            preview::MdBlock::CodeBlock { lines } => div()
                .my_1()
                .p_2()
                .rounded_md()
                .bg(rgba_alpha(theme.tab_bar_background, 0.9))
                .flex()
                .flex_col()
                .children(lines.iter().map(|line| self.preview_code_line(line, None)))
                .into_any_element(),
            preview::MdBlock::Quote { spans } => {
                let (text, highlights) = self.preview_md_text(spans);
                div()
                    .my_1()
                    .pl_2()
                    .border_l_2()
                    .border_color(hsla_alpha(theme.accent, 0.6))
                    .text_color(hsla_alpha(theme.foreground, 0.75))
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .into_any_element()
            }
            preview::MdBlock::Rule => div()
                .my_2()
                .h(px(1.0))
                .bg(hsla_alpha(theme.pane_border, 0.9))
                .into_any_element(),
        }
    }
}

/// tako-control の dispatch がドメイン状態へ触るためのホスト実装。
/// セッション起動だけは GPUI の Context が要るため `pending_attach` へ積み、
/// dispatch 直後（IPC リクエストループ内）で実行する
impl ControlHost for TakoApp {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }

    fn session(&self, pane: PaneId) -> Option<&TerminalSession> {
        self.terminals.get(&pane)
    }

    fn attach_session(&mut self, pane: PaneId, options: SpawnOptions) {
        self.pending_attach.push((pane, options));
    }

    fn detach_session(&mut self, pane: PaneId) {
        self.terminals.remove(&pane);
        self.previews.remove(&pane);
        self.scroll_accum.remove(&pane);
        self.scroll_ctls.remove(&pane);
        // CLI / MCP からの明示 close（FR-2.5.4）。バックエンドセッションも片付ける
        self.drop_backend_session(pane);
    }

    fn auto_rename_enabled(&self) -> bool {
        self.autorename.enabled
    }

    fn set_auto_rename(&mut self, enabled: bool) {
        self.autorename.enabled = enabled;
        // 永続化（FR-2.12.4）。セルフテスト中はユーザー設定を汚さない
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.auto_rename = enabled;
            if let Err(e) = tako_control::settings::save(&settings) {
                eprintln!("warning: 設定を保存できない: {e}");
            }
        }
    }

    fn port_detect_enabled(&self) -> bool {
        self.port_detect
    }

    fn set_port_detect(&mut self, enabled: bool) {
        self.port_detect = enabled;
        if !enabled {
            // 検知済み情報も掃除する（list の listen_ports / 表示中チップ / 却下記録）
            for session in self.terminals.values_mut() {
                session.set_listen_ports(Vec::new());
            }
            self.port_suggestions.clear();
            self.dismissed_ports.clear();
        }
        // 永続化（FR-2.4.4）。セルフテスト中はユーザー設定を汚さない
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.port_detect = enabled;
            if let Err(e) = tako_control::settings::save(&settings) {
                eprintln!("warning: 設定を保存できない: {e}");
            }
        }
    }

    fn tmux_persist_enabled(&self) -> bool {
        self.tmux_persist
    }

    fn set_tmux_persist(&mut self, enabled: bool) {
        self.tmux_persist = enabled;
        if enabled && tako_core::tmux_backend::available() {
            // 過去の起動から生き残っているサーバーがあれば最新 conf を再適用する
            tako_core::tmux_backend::sync_conf(&tako_core::tmux_backend::socket_name());
        }
        // 切替は以後生成されるペインに効く。既存のバックエンドペインはそのまま
        // （close 時の kill は backend_sessions に残っているため引き続き行われる）。
        // 永続化（FR-5）。セルフテスト中はユーザー設定・レイアウトを汚さない
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.tmux_persist = enabled;
            if let Err(e) = tako_control::settings::save(&settings) {
                eprintln!("warning: 設定を保存できない: {e}");
            }
            if !enabled {
                // OFF 中は復元しない。次回起動が古いレイアウトを拾わないよう消しておく
                tako_control::layout::remove();
                self.last_saved_layout = None;
            }
        }
    }

    fn backend_session(&self, pane: PaneId) -> Option<String> {
        self.backend_sessions.get(&pane).cloned()
    }

    fn panel_state(&self) -> (bool, f32, tako_control::protocol::PanelViewWire) {
        let view = match self.panel_view {
            PanelView::Tmux => tako_control::protocol::PanelViewWire::Tmux,
            PanelView::Git => tako_control::protocol::PanelViewWire::Git,
        };
        (self.panel_visible, self.panel_width, view)
    }

    fn set_panel(
        &mut self,
        visible: Option<bool>,
        width: Option<f32>,
        view: Option<tako_control::protocol::PanelViewWire>,
    ) {
        if let Some(visible) = visible {
            self.panel_visible = visible;
        }
        if let Some(width) = width {
            self.panel_width = width.max(PANEL_MIN_WIDTH);
        }
        if let Some(view) = view {
            self.panel_view = match view {
                tako_control::protocol::PanelViewWire::Tmux => PanelView::Tmux,
                tako_control::protocol::PanelViewWire::Git => PanelView::Git,
            };
        }
        // tmux ビューを開いたら一覧を即時更新する（描画通知は dispatch ループが行う）
        if self.panel_visible && self.panel_view == PanelView::Tmux {
            self.refresh_tmux_data();
        }
    }

    fn filetree_visible(&self) -> bool {
        self.filetree.visible
    }

    fn set_filetree(&mut self, visible: bool) {
        if self.filetree.visible != visible {
            self.toggle_filetree();
        }
    }

    fn preview_state(
        &self,
        pane: PaneId,
    ) -> Option<(String, tako_control::protocol::PreviewModeWire)> {
        self.previews
            .get(&pane)
            .map(|p| (p.path.display().to_string(), p.mode.to_wire()))
    }

    fn set_preview(
        &mut self,
        pane: PaneId,
        path: &str,
        mode: tako_control::protocol::PreviewModeWire,
    ) {
        self.previews.insert(
            pane,
            preview::load(
                std::path::Path::new(path),
                preview::PreviewMode::from_wire(mode),
            ),
        );
    }

    fn preview_pane_of_tab(&self, tab: TabId) -> Option<PaneId> {
        self.workspace
            .get_tab(tab)?
            .tree()
            .panes()
            .into_iter()
            .map(|p| p.id())
            .find(|id| self.previews.contains_key(id))
    }
}

/// IME（macOS では NSTextInputClient 相当）との接点（FR-1.9）。
/// ターミナルには編集対象の「文書」が無いため、**未確定文字列そのものを擬似ドキュメント**
/// として公開する（範囲はすべてその文字列内の UTF-16 オフセット）。
/// 確定文字列は PTY へ書き、未確定文字列は render のオーバーレイでカーソル位置に表示する
impl EntityInputHandler for TakoApp {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let ime = self.ime.as_ref()?;
        let start = utf16_to_byte_offset(&ime.text, range_utf16.start);
        let end = utf16_to_byte_offset(&ime.text, range_utf16.end);
        Some(ime.text.get(start..end)?.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // 変換中は IME の注目文節（無ければ末尾キャレット）、非変換中は空ドキュメントの先頭
        let range = match self.ime.as_ref() {
            Some(ime) => ime.selected_utf16.clone().unwrap_or_else(|| {
                let end = utf16_len(&ime.text);
                end..end
            }),
            None => 0..0,
        };
        Some(UTF16Selection {
            range,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime.as_ref().map(|ime| 0..utf16_len(&ime.text))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // NSTextInputClient の規約: unmark は「未確定文字列をそのまま挿入扱いにする」
        if let Some(ime) = self.ime.take() {
            if let Some(session) = self.terminals.get(&ime.pane) {
                session.write(ime.text.into_bytes());
            }
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 確定（insertText 相当）。変換を開始したペインへ書き、変換状態を畳む
        let pane = self
            .ime
            .take()
            .map(|ime| ime.pane)
            .unwrap_or_else(|| self.focused_pane());
        self.cancel_scroll_before_input(pane);
        if let Some(session) = self.terminals.get(&pane) {
            session.clear_selection();
            session.write(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // IME は毎回未確定文字列の全文を渡してくるので丸ごと差し替える。
        // 空文字は変換キャンセル（esc）を意味する
        if new_text.is_empty() {
            self.ime = None;
        } else {
            let pane = self
                .ime
                .take()
                .map(|ime| ime.pane)
                .unwrap_or_else(|| self.focused_pane());
            self.ime = Some(ImeComposition {
                pane,
                text: new_text.to_string(),
                selected_utf16: new_selected_range,
            });
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 変換候補ウィンドウの位置出し。カーソルセル + 範囲先頭までの描画幅
        let origin = self.pane_cursor_origin(self.ime_target(), window)?;
        let cell = self.cell_size?;
        let x_offset = match self.ime.as_ref() {
            Some(ime) => {
                let start = clamp_ime_range_start(
                    range_utf16.start,
                    ime.text.encode_utf16().count(),
                    ime.selected_utf16.as_ref(),
                );
                let end = utf16_to_byte_offset(&ime.text, start);
                self.ime_prefix_width(&ime.text[..end], window)
            }
            None => px(0.0),
        };
        Some(Bounds::new(
            point(origin.x + x_offset, origin.y),
            size(cell.width, cell.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// 文字数ベースの単純な切り詰め（タブ表示名用）
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// GPUI の Keystroke を端末入力バイト列へ変換する
/// `firstRectForCharacterRange` の range 先頭を擬似ドキュメント（marked text のみ・
/// 0 起点）内へ解釈する。macOS のライブ変換は確定済みテキストを含む**文書全体基準**の
/// オフセットを渡してくることがあり、そのまま使うと候補ウィンドウが確定分の幅だけ
/// 右へずれ続ける（打ち進めるほど離れる）。範囲外のオフセットは注目文節の先頭
/// （無ければ末尾 = 挿入点）として扱う
fn clamp_ime_range_start(
    start_utf16: usize,
    marked_utf16_len: usize,
    selected: Option<&Range<usize>>,
) -> usize {
    if start_utf16 <= marked_utf16_len {
        start_utf16
    } else {
        selected.map(|r| r.start).unwrap_or(marked_utf16_len)
    }
}

/// 修飾キーのエンコード（xterm / kitty 共通: 1 + shift | alt<<1 | ctrl<<2 | super<<3）
fn encode_modifiers(m: &Modifiers) -> u8 {
    1 + (m.shift as u8)
        + ((m.alt as u8) << 1)
        + ((m.control as u8) << 2)
        + ((m.platform as u8) << 3)
}

/// CSI u（kitty keyboard protocol）の送出範囲
#[derive(Clone, Copy, PartialEq, Eq)]
enum CsiUMode {
    /// レガシー端末モード（CSI u を送らない）
    Off,
    /// 修飾付き Enter / Tab / Backspace / Esc のみ CSI u（tmux バックエンドペイン）。
    /// Esc 単押しは素の \e のまま — tmux 3.6 は受信した CSI 27u を内側ペインの
    /// kitty 要求の有無に関係なく素通しするため、CSI u 非対応アプリの入力欄に
    /// 「27u」が文字として挿入される（2026-06-12 実機バグ）。修飾付きキーは
    /// レガシー形式だと区別不能（Shift+Enter = \r）なので CSI u を維持する
    ModifiedOnly,
    /// Esc 単押しも CSI 27u（アプリ自身が kitty disambiguate を要求済み = 確実に解釈できる）
    Full,
}

/// キー入力 → PTY バイト列。`csi_u` は kitty keyboard protocol（disambiguate
/// フラグ。TUI が `CSI > 1 u` で有効化。Claude Code 等が Shift+Enter を
/// 区別するために使う）の送出範囲。
/// それ以外のフラグ（REPORT_ALL_KEYS 等）は未対応（必要になったら拡張する）
fn keystroke_to_bytes(ks: &Keystroke, csi_u: CsiUMode) -> Option<Vec<u8>> {
    let mods = encode_modifiers(&ks.modifiers);
    if csi_u != CsiUMode::Off {
        let code: Option<u32> = match ks.key.as_str() {
            "escape" if csi_u == CsiUMode::Full || mods > 1 => Some(27),
            "enter" if mods > 1 => Some(13),
            "tab" if mods > 1 => Some(9),
            "backspace" if mods > 1 => Some(127),
            _ => None,
        };
        if let Some(code) = code {
            return Some(if mods > 1 {
                format!("\x1b[{code};{mods}u").into_bytes()
            } else {
                format!("\x1b[{code}u").into_bytes()
            });
        }
    }
    // Ctrl+英字 → C0 制御コード
    if ks.modifiers.control {
        let mut chars = ks.key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(vec![(c.to_ascii_lowercase() as u8) & 0x1f]);
            }
        }
    }
    // 機能キー。修飾付きは xterm 標準の CSI 1;mod X / CSI n;mod ~ 形式
    let csi_letter = |letter: char| -> Vec<u8> {
        if mods > 1 {
            format!("\x1b[1;{mods}{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if mods > 1 {
            format!("\x1b[{n};{mods}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let bytes: Vec<u8> = match ks.key.as_str() {
        "enter" => b"\r".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => csi_letter('A'),
        "down" => csi_letter('B'),
        "right" => csi_letter('C'),
        "left" => csi_letter('D'),
        "home" => csi_letter('H'),
        "end" => csi_letter('F'),
        "pageup" => csi_tilde(5),
        "pagedown" => csi_tilde(6),
        "delete" => csi_tilde(3),
        _ => {
            // 印字可能文字は key_char をそのまま送る（IME 確定文字列もここに来る）
            let ch = ks.key_char.as_ref()?;
            if ch.is_empty() {
                return None;
            }
            return Some(ch.as_bytes().to_vec());
        }
    };
    Some(bytes)
}

impl Render for TakoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cell = self.measure_cell(window);
        let theme = self.theme.clone();

        // ファイルツリーの root をフォーカスペインの cwd に追従させる（FR-3.1）
        self.sync_filetree_roots();

        // OS ウィンドウのフレームを採取する（layout 保存 = 再起動時のウィンドウ復元用。
        // window_bounds() は fullscreen / maximized 中でも復元サイズを返す）
        self.window_frame = Some({
            let (state, bounds) = match window.window_bounds() {
                WindowBounds::Windowed(b) => ("windowed", b),
                WindowBounds::Maximized(b) => ("maximized", b),
                WindowBounds::Fullscreen(b) => ("fullscreen", b),
            };
            tako_control::layout::WindowFrame {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                state: state.to_string(),
            }
        });

        // アクティブタブのレイアウト（単位矩形）と、マウス変換用のピクセル矩形を更新する。
        // サイドバー表示中はその幅だけコンテンツ領域を右へずらす（ペイン矩形・境界
        // ハンドル・IME 位置はすべて content_origin / content_size 起点で連動する）
        let viewport = window.viewport_size();
        let sidebar_width = if self.filetree.visible {
            px(SIDEBAR_WIDTH)
        } else {
            px(0.0)
        };
        // 右サイドバー情報パネルの幅もコンテンツ領域から差し引く
        let panel_width = if self.panel_visible {
            px(self.panel_width)
        } else {
            px(0.0)
        };
        let content_origin = point(sidebar_width, px(TAB_BAR_HEIGHT));
        // 下部ステータスバー（FR-2.16.4）の分も差し引く
        let content_size = size(
            viewport.width - sidebar_width - panel_width,
            viewport.height - px(TAB_BAR_HEIGHT) - px(STATUS_BAR_HEIGHT),
        );
        let tree = self.workspace.active_tab().tree();
        let focused = tree.focused();
        let layout = tree.layout(Rect::UNIT);
        self.pane_text_areas = layout
            .iter()
            .map(|(id, r)| {
                let inset = PANE_BORDER + PANE_PADDING;
                // テキスト領域はタイトルバー（PANE_TITLE_BAR）の下から始まる
                let origin = point(
                    content_origin.x + content_size.width * r.x + px(inset),
                    content_origin.y + content_size.height * r.y + px(inset + PANE_TITLE_BAR),
                );
                let area_size = size(
                    content_size.width * r.width - px(inset * 2.0),
                    content_size.height * r.height - px(inset * 2.0 + PANE_TITLE_BAR),
                );
                (*id, Bounds::new(origin, area_size))
            })
            .collect();

        let panes: Vec<_> = layout
            .into_iter()
            .map(|(id, rect)| {
                let area = self
                    .pane_text_areas
                    .iter()
                    .find(|(p, _)| *p == id)
                    .map(|(_, b)| *b)
                    .expect("直前に同じ layout から構築済み");
                self.render_pane(id, rect, area, id == focused, cx)
            })
            .collect();
        let _ = cell;

        // ペイン境界のドラッグハンドル（仕切り線の上に数 px 幅の透明な当たり領域を重ねる）。
        // ヒットテストとカーソル形状は gpui に任せ、押下で start_border_drag を呼ぶ。
        // 境界座標はウィンドウ空間で算出し、配置はコンテナ（y=TAB_BAR_HEIGHT 起点）ローカルへ直す
        let border_rect = Rect::new(
            f32::from(content_origin.x),
            f32::from(content_origin.y),
            f32::from(content_size.width),
            f32::from(content_size.height),
        );
        let origin_x = f32::from(content_origin.x);
        let origin_y = f32::from(content_origin.y);
        let border_handles: Vec<_> = self
            .workspace
            .active_tab()
            .tree()
            .borders(border_rect)
            .into_iter()
            .map(|b| {
                let drag = DragBorder {
                    index: b.index,
                    axis: b.axis,
                    area: b.area,
                };
                let len = b.span_end - b.span_start;
                let base = div().absolute().occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.start_border_drag(drag, cx);
                        cx.stop_propagation();
                    }),
                );
                match b.axis {
                    // 縦線（左右ドラッグ）
                    SplitAxis::Horizontal => base
                        .left(px(b.position - origin_x - BORDER_HANDLE / 2.0))
                        .top(px(b.span_start - origin_y))
                        .w(px(BORDER_HANDLE))
                        .h(px(len))
                        .cursor(CursorStyle::ResizeLeftRight),
                    // 横線（上下ドラッグ）
                    SplitAxis::Vertical => base
                        .left(px(b.span_start - origin_x))
                        .top(px(b.position - origin_y - BORDER_HANDLE / 2.0))
                        .w(px(len))
                        .h(px(BORDER_HANDLE))
                        .cursor(CursorStyle::ResizeUpDown),
                }
            })
            .collect();

        // IME 変換中テキストのインライン表示（FR-1.9）。変換対象ペインのカーソル位置に
        // 未確定文字列を重ね、全体に細下線・IME の注目文節に太下線 + 選択色を付ける
        let ime_overlay = self.ime.as_ref().and_then(|ime| {
            let anchor = self.pane_cursor_origin(ime.pane, window)?;
            let text = ime.text.clone();
            // ハイライト範囲は重複禁止（StyledText の要求）のため、注目文節の前・文節・後の
            // 3 区間に分割して組む。文節範囲（UTF-16）はバイト範囲へ変換する
            let thin = HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: None,
                    wavy: false,
                }),
                ..HighlightStyle::default()
            };
            let thick = HighlightStyle {
                background_color: Some(hsla(theme.selection_background)),
                underline: Some(UnderlineStyle {
                    thickness: px(2.0),
                    color: Some(hsla(theme.accent)),
                    wavy: false,
                }),
                ..HighlightStyle::default()
            };
            let clause = ime
                .selected_utf16
                .as_ref()
                .map(|sel| {
                    utf16_to_byte_offset(&text, sel.start)..utf16_to_byte_offset(&text, sel.end)
                })
                .filter(|r| !r.is_empty())
                .unwrap_or(0..0);
            let mut highlights = Vec::new();
            if clause.start > 0 {
                highlights.push((0..clause.start, thin));
            }
            if !clause.is_empty() {
                highlights.push((clause.clone(), thick));
            }
            if clause.end < text.len() {
                highlights.push((clause.end..text.len(), thin));
            }
            Some(
                div()
                    .absolute()
                    .left(anchor.x - content_origin.x)
                    .top(anchor.y - content_origin.y)
                    .h(px(theme.line_height))
                    .bg(rgba(theme.background))
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    ),
            )
        });

        // IME（確定・未確定入力）の受け口を OS へ登録する。`Window::handle_input` は
        // paint フェーズ限定 API のため、何も描かない canvas の paint フックから呼ぶ
        let ime_registration = {
            let entity = cx.entity();
            let focus = self.focus_handle.clone();
            let target = self.ime_target();
            let target_bounds = self
                .pane_text_areas
                .iter()
                .find(|(id, _)| *id == target)
                .map(|(_, b)| *b)
                .unwrap_or_else(|| Bounds::new(content_origin, content_size));
            canvas(
                |_, _, _| (),
                move |_, _, window, cx| {
                    window.handle_input(
                        &focus,
                        ElementInputHandler::new(target_bounds, entity),
                        cx,
                    );
                },
            )
            .absolute()
            .size_full()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgba(theme.background))
            .font_family(theme.font_family.clone())
            .text_size(px(theme.font_size))
            .track_focus(&self.focus_handle)
            .key_context("TakoApp")
            .on_action(
                cx.listener(|this, _: &SplitRight, _, cx| this.split(SplitDirection::Right, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SplitDown, _, cx| this.split(SplitDirection::Down, cx)),
            )
            .on_action(cx.listener(|this, _: &ClosePane, _, cx| this.close_focused_pane(cx)))
            .on_action(cx.listener(|this, _: &NewTab, _, cx| this.new_tab(cx)))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| {
                this.workspace.activate_next_tab();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PrevTab, _, cx| {
                this.workspace.activate_prev_tab();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusLeft, _, cx| {
                this.focus_direction(SplitDirection::Left, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusRight, _, cx| {
                this.focus_direction(SplitDirection::Right, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusUp, _, cx| {
                    this.focus_direction(SplitDirection::Up, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusDown, _, cx| {
                this.focus_direction(SplitDirection::Down, cx)
            }))
            .on_action(cx.listener(|this, _: &WidenPane, _, cx| {
                this.resize_focused(SplitAxis::Horizontal, RESIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &NarrowPane, _, cx| {
                this.resize_focused(SplitAxis::Horizontal, -RESIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &TallenPane, _, cx| {
                this.resize_focused(SplitAxis::Vertical, RESIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &ShortenPane, _, cx| {
                this.resize_focused(SplitAxis::Vertical, -RESIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &PasteClipboard, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.toggle_filetree();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                // 終了直前の構成を保存してから抜ける（Phase 5.5。セッションは残る = 永続化）。
                // 接続情報は片付け、死んだ接続先を CLI の候補に残さない（バグ (8)）
                this.save_layout();
                tako_control::discovery::cleanup(std::process::id());
                cx.quit()
            }))
            .on_action(cx.listener(|this, _: &ActivateTab1, _, cx| this.activate_tab_index(0, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab2, _, cx| this.activate_tab_index(1, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab3, _, cx| this.activate_tab_index(2, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab4, _, cx| this.activate_tab_index(3, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab5, _, cx| this.activate_tab_index(4, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab6, _, cx| this.activate_tab_index(5, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab7, _, cx| this.activate_tab_index(6, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab8, _, cx| this.activate_tab_index(7, cx)))
            .on_action(cx.listener(|this, _: &ActivateTab9, _, cx| this.activate_tab_index(8, cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.handle_key(&event.keystroke, cx);
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.on_mouse_up(event, cx);
                }),
            )
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .flex_1()
                    // ステータスバー消失の根本対策（2026-06-13 実機）: flex 子の自動最小
                    // サイズは overflow: visible だと min-content になるため、サイドバー /
                    // パネルの内在コンテンツ高（ファイルツリー行数・tmux 一覧）がウィンドウ高を
                    // 超えるとこの中段が縮めず、下のステータスバーが画面外へ押し出されていた。
                    // min_h(0) で常にビューポート内へ収める（内側のスクロールが内容を受ける）
                    .min_h(px(0.0))
                    .flex()
                    .flex_row()
                    .children(self.render_sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .relative()
                            .children(panes)
                            .children(border_handles)
                            .children(ime_overlay),
                    )
                    .children(self.render_panel(cx)),
            )
            .child(self.render_status_bar(cx))
            .child(ime_registration)
    }
}

fn main() {
    // セルフテストの tmux バックエンド項目は、ユーザーの実バックエンド（tako サーバー）を
    // 汚さない隔離ソケットで行う（終了時に self_test 側が kill-server で片付ける）
    if std::env::var_os("TAKO_SELF_TEST").is_some()
        && std::env::var_os("TAKO_TMUX_SOCKET").is_none()
    {
        std::env::set_var(
            "TAKO_TMUX_SOCKET",
            format!("tako-st-{}", std::process::id()),
        );
    }
    // セルフテストの接続情報はメインの control.json に**触らない**（2026-06-12 バグ (8):
    // 一時インスタンスの上書き → exit でメインへの CLI / MCP 接続が全断した）。
    // 専用一時ディレクトリへ隔離し、ペイン内 CLI も env 継承で同じ場所を見る
    if std::env::var_os("TAKO_SELF_TEST").is_some()
        && std::env::var_os("TAKO_DISCOVERY_DIR").is_none()
    {
        std::env::set_var(
            "TAKO_DISCOVERY_DIR",
            std::env::temp_dir().join(format!("tako-st-discovery-{}", std::process::id())),
        );
    }
    application().run(|cx: &mut App| {
        cx.bind_keys(key_bindings());
        // 保存済みウィンドウフレームの復元（FR-5。終了前にフルスクリーンなら
        // フルスクリーンで開く）。セルフテストは既定サイズで決定的に動かす
        let saved_frame = if std::env::var_os("TAKO_SELF_TEST").is_none() {
            tako_control::layout::load().and_then(|l| l.window)
        } else {
            None
        };
        let window_bounds = match saved_frame {
            // 壊れた保存値（極端に小さい等）は既定へフォールバック
            Some(f) if f.width >= 200.0 && f.height >= 150.0 => {
                let bounds = Bounds::new(point(px(f.x), px(f.y)), size(px(f.width), px(f.height)));
                match f.state.as_str() {
                    "fullscreen" => WindowBounds::Fullscreen(bounds),
                    "maximized" => WindowBounds::Maximized(bounds),
                    _ => WindowBounds::Windowed(bounds),
                }
            }
            _ => WindowBounds::Windowed(Bounds::centered(None, size(px(960.), px(600.)), cx)),
        };
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(window_bounds),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(TakoApp::new);
                    window.focus(&view.read(cx).focus_handle.clone(), cx);
                    view
                },
            )
            .expect("ウィンドウを開けなかった");
        cx.activate(true);

        if std::env::var_os("TAKO_SELF_TEST").is_some() {
            self_test::run(window, cx);
        }
    });
}

/// セルフテスト: キーディスパッチ経由で入力・分割・フォーカス・リサイズ・タブ・色・
/// スクロールバック・コピペの経路（1〜13）と、制御プレーンの環境変数注入 +
/// ペイン内シェルから実 `tako` CLI を叩く e2e（14〜29）、内蔵 MCP サーバー
/// （Streamable HTTP + stdio ブリッジ、30〜36）、IME 変換状態（37〜39）を機械検証して終了する。
/// `WindowHandle<V>::update` 内の dispatch_keystroke はルートビューの二重借用で
/// パニックするため AnyWindowHandle::update を使う（poc/README.md）
mod self_test {
    use super::*;
    use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};

    fn fail(step: &str) -> ! {
        println!("TAKO_APP_SELF_TEST_FAILED: {step}");
        std::process::exit(1);
    }

    fn check(cond: bool, step: &str) {
        if !cond {
            fail(step);
        }
    }

    /// 文字列をキーストローク列としてウィンドウへ流し込む
    fn type_text(any: AnyWindowHandle, cx: &mut AsyncApp, text: &str, enter: bool) {
        let text = text.to_string();
        let _ = any.update(cx, |_, window, cx| {
            for ch in text.chars() {
                let keystroke = Keystroke {
                    modifiers: Modifiers::default(),
                    key: ch.to_string(),
                    key_char: Some(ch.to_string()),
                };
                window.dispatch_keystroke(keystroke, cx);
            }
            if enter {
                // 固定文字列のパースは失敗しない（論理的に到達不能）
                window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
            }
        });
    }

    fn press(any: AnyWindowHandle, cx: &mut AsyncApp, combo: &str) {
        let combo = combo.to_string();
        let _ = any.update(cx, |_, window, cx| {
            window.dispatch_keystroke(
                Keystroke::parse(&combo).expect("セルフテストのキー表記は固定"),
                cx,
            );
        });
    }

    /// 最小 HTTP クライアント（MCP Streamable HTTP の機械検証用）。(status, body) を返す
    fn mcp_post(
        url: &str,
        token: Option<&str>,
        extra_headers: &[(&str, &str)],
        body: &str,
    ) -> Option<(u16, String)> {
        use std::io::{Read, Write};
        let rest = url.strip_prefix("http://")?;
        let (hostport, path) = rest.split_once('/')?;
        let mut stream = std::net::TcpStream::connect(hostport).ok()?;
        let mut request = format!(
            "POST /{path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        if let Some(token) = token {
            request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        for (name, value) in extra_headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream.write_all(request.as_bytes()).ok()?;
        let mut response = String::new();
        stream.read_to_string(&mut response).ok()?;
        let status = response.split_whitespace().nth(1)?.parse().ok()?;
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        Some((status, body))
    }

    /// [`mcp_post`] をバックグラウンドスレッドで実行する。このセルフテスト future は
    /// メインスレッド（foreground executor）で動いており、ここで同期ブロックすると
    /// 同じスレッドの dispatch ループが止まり tools/call の応答待ちとデッドロックする
    async fn mcp_post_bg(
        cx: &AsyncApp,
        url: &str,
        token: Option<&str>,
        extra_headers: &[(&str, &str)],
        body: &str,
    ) -> Option<(u16, String)> {
        let url = url.to_string();
        let token = token.map(str::to_string);
        let headers: Vec<(String, String)> = extra_headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let body = body.to_string();
        cx.background_executor()
            .spawn(async move {
                let headers: Vec<(&str, &str)> = headers
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                mcp_post(&url, token.as_deref(), &headers, &body)
            })
            .await
    }

    /// [`bridge_roundtrip`] のバックグラウンド版（デッドロック回避は mcp_post_bg と同じ理由。
    /// ブリッジの tools/call は IPC 経由で UI スレッドの dispatch を待つ）
    async fn bridge_roundtrip_bg(
        cx: &AsyncApp,
        cli: std::path::PathBuf,
        envs: Vec<(String, String)>,
        inputs: &[&'static str],
    ) -> Vec<String> {
        let inputs = inputs.to_vec();
        cx.background_executor()
            .spawn(async move {
                let envs: Vec<(&str, &str)> =
                    envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                bridge_roundtrip(&cli, &envs, &inputs)
            })
            .await
    }

    /// stdio ブリッジ（`tako mcp serve`）へ MCP メッセージ列を流し、応答行を回収する。
    /// 指定した TAKO_* 以外は環境から除去し、tako 内 / 外の両状態を再現できるようにする
    fn bridge_roundtrip(
        cli: &std::path::Path,
        envs: &[(&str, &str)],
        inputs: &[&str],
    ) -> Vec<String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut command = Command::new(cli);
        command
            .args(["mcp", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for key in [
            "TAKO_SOCKET",
            "TAKO_TOKEN",
            "TAKO_PANE_ID",
            "TAKO_TAB_ID",
            "TAKO_MCP_URL",
        ] {
            command.env_remove(key);
        }
        for (key, value) in envs {
            command.env(key, value);
        }
        let Ok(mut child) = command.spawn() else {
            return Vec::new();
        };
        if let Some(mut stdin) = child.stdin.take() {
            for line in inputs {
                let _ = writeln!(stdin, "{line}");
            }
            // drop で stdin が閉じ、ブリッジは EOF で終了する
        }
        let Ok(output) = child.wait_with_output() else {
            return Vec::new();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect()
    }

    /// フォーカス中ペインの表示行に needle が含まれるか
    fn focused_contains(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, needle: &str) -> bool {
        window
            .update(cx, |app, _, _| {
                app.focused_session()
                    .map(|s| s.visible_lines().iter().any(|l| l.contains(needle)))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    pub fn run(window: WindowHandle<TakoApp>, cx: &mut App) {
        cx.spawn(async move |cx| {
            let any: AnyWindowHandle = window.into();
            let wait = |cx: &mut AsyncApp, ms: u64| {
                cx.background_executor().timer(Duration::from_millis(ms))
            };

            // 1. 起動 + 素の入力経路
            wait(cx, 2500).await;
            type_text(any, cx, "echo TAKO-INPUT-OK", true);
            wait(cx, 1000).await;
            check(focused_contains(window, cx, "TAKO-INPUT-OK"), "入力エコー");

            // 1b. TERM / COLORTERM 注入（tmux 等の「missing or unsuitable terminal」回避）
            type_text(any, cx, "echo TERMCHK=$TERM,$COLORTERM", true);
            wait(cx, 800).await;
            check(
                focused_contains(window, cx, "TERMCHK=xterm-256color,truecolor"),
                "TERM / COLORTERM 注入",
            );

            // 1c. 初期 cwd はホーム（.app 起動時に `/` へ落ちない）
            type_text(any, cx, "[ \"$PWD\" = \"$HOME\" ] && echo CWDCHK-$((40+2))", true);
            wait(cx, 800).await;
            check(focused_contains(window, cx, "CWDCHK-42"), "初期 cwd はホーム");

            // 1d. tako 内で tmux がエラーなく起動できる（TERM 修正の実地確認。
            //     専用ソケット -L で実環境の tmux サーバーに触れない。未インストール時は素通し）
            type_text(
                any,
                cx,
                "if command -v tmux >/dev/null; then tmux -L takoST kill-server 2>/dev/null; \
                 tmux -L takoST new-session -d 'sleep 5' && tmux -L takoST kill-server 2>/dev/null \
                 && echo TMUX-OK-42; else echo TMUX-OK-42; fi",
                true,
            );
            wait(cx, 1500).await;
            check(
                focused_contains(window, cx, "TMUX-OK-42"),
                "tako 内で tmux がエラーなく起動",
            );

            // 1e. Backspace が \x7f を送り行編集で文字が消える（特殊キー→PTY バイト変換の往復確認）。
            //     "echo BSPxx" から 2 文字消して "OK" を足し "echo BSPOK" になる。
            //     もし空白等が送られると行が一致せず出力が変わる
            type_text(any, cx, "echo BSPxx", false);
            press(any, cx, "backspace");
            press(any, cx, "backspace");
            type_text(any, cx, "OK", true);
            wait(cx, 900).await;
            check(focused_contains(window, cx, "BSPOK"), "Backspace で行編集できる");

            let pane1 = window
                .update(cx, |app, _, _| app.focused_pane())
                .unwrap_or_else(|_| fail("初期ペイン取得"));

            // 2. cmd-d 縦分割（右に生える）
            press(any, cx, "cmd-d");
            wait(cx, 1500).await;
            let (pane_count, terminal_count, pane2) = window
                .update(cx, |app, _, _| {
                    (
                        app.workspace.active_tab().tree().len(),
                        app.terminals.len(),
                        app.focused_pane(),
                    )
                })
                .unwrap_or_else(|_| fail("分割後の状態取得"));
            check(pane_count == 2 && terminal_count == 2, "cmd-d で 2 ペイン");
            check(pane2 != pane1, "分割後フォーカスは新ペイン");

            // 3. 新ペインだけに入力が流れる
            type_text(any, cx, "echo TAKO-PANE2-OK", true);
            wait(cx, 1000).await;
            check(
                focused_contains(window, cx, "TAKO-PANE2-OK"),
                "ペイン 2 へ入力",
            );
            let pane1_clean = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&pane1)
                        .map(|s| {
                            !s.visible_lines()
                                .iter()
                                .any(|l| l.contains("TAKO-PANE2-OK"))
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(pane1_clean, "ペイン 1 に漏れない");

            // 4. 方向フォーカス移動
            press(any, cx, "cmd-alt-left");
            let refocused = window
                .update(cx, |app, _, _| app.focused_pane())
                .unwrap_or_else(|_| fail("フォーカス移動後の取得"));
            check(refocused == pane1, "cmd-alt-left で左ペインへ");

            // 5. キーボードリサイズ（フォーカスペインの横取り分が増える）
            press(any, cx, "ctrl-cmd-right");
            let width = window
                .update(cx, |app, _, _| {
                    app.workspace
                        .active_tab()
                        .tree()
                        .layout(Rect::UNIT)
                        .into_iter()
                        .find(|(id, _)| *id == pane1)
                        .map(|(_, r)| r.width)
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0);
            check(width > 0.52, "ctrl-cmd-right でリサイズ");

            // 5b. 境界ドラッグでリサイズ（border ヒットテスト→座標を比率へ換算→set_split_ratio）。
            //     pane1|pane2 の縦境界を領域の左から 30% へドラッグし、pane1 幅が約 0.3 になる。
            //     ドラッグ終了でドラッグ状態がクリアされることも確認する
            let (drag_width, drag_cleared) = window
                .update(cx, |app, win, cx| {
                    let vp = win.viewport_size();
                    let area_w = f32::from(vp.width);
                    let area_h = f32::from(vp.height) - TAB_BAR_HEIGHT;
                    let border_rect = Rect::new(0.0, TAB_BAR_HEIGHT, area_w, area_h);
                    let border = app
                        .workspace
                        .active_tab()
                        .tree()
                        .borders(border_rect)
                        .into_iter()
                        .find(|b| b.axis == SplitAxis::Horizontal)
                        .expect("縦境界が 1 本あるはず");
                    app.dragging_border = Some(DragBorder {
                        index: border.index,
                        axis: border.axis,
                        area: border.area,
                    });
                    let drag_x = border.area.x + border.area.width * 0.3;
                    let drag_y = border.area.y + border.area.height * 0.5;
                    let pos = point(px(drag_x), px(drag_y));
                    app.on_mouse_move(
                        &MouseMoveEvent {
                            position: pos,
                            pressed_button: Some(MouseButton::Left),
                            modifiers: Modifiers::default(),
                        },
                        win,
                        cx,
                    );
                    app.on_mouse_up(
                        &MouseUpEvent {
                            button: MouseButton::Left,
                            position: pos,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        },
                        cx,
                    );
                    let w = app
                        .workspace
                        .active_tab()
                        .tree()
                        .layout(Rect::UNIT)
                        .into_iter()
                        .find(|(id, _)| *id == pane1)
                        .map(|(_, r)| r.width)
                        .unwrap_or(0.0);
                    (w, app.dragging_border.is_none())
                })
                .unwrap_or((0.0, false));
            check(
                (drag_width - 0.3).abs() < 0.02 && drag_cleared,
                "境界ドラッグでリサイズ",
            );

            // 5c. 【回帰】ウィンドウ外リリース等で MouseUp を取りこぼしても、ボタン非押下の
            //     移動でドラッグ状態が畳まれる（残留すると当たり判定が広がったように見える）
            let stale_cleared = window
                .update(cx, |app, win, cx| {
                    let vp = win.viewport_size();
                    let area_w = f32::from(vp.width);
                    let area_h = f32::from(vp.height) - TAB_BAR_HEIGHT;
                    let border_rect = Rect::new(0.0, TAB_BAR_HEIGHT, area_w, area_h);
                    let border = app
                        .workspace
                        .active_tab()
                        .tree()
                        .borders(border_rect)
                        .into_iter()
                        .find(|b| b.axis == SplitAxis::Horizontal)
                        .expect("縦境界が 1 本あるはず");
                    // MouseUp を挟まずドラッグ状態だけ残す（ウィンドウ外リリースの再現）
                    app.dragging_border = Some(DragBorder {
                        index: border.index,
                        axis: border.axis,
                        area: border.area,
                    });
                    app.on_mouse_move(
                        &MouseMoveEvent {
                            position: point(px(10.0), px(TAB_BAR_HEIGHT + 10.0)),
                            pressed_button: None,
                            modifiers: Modifiers::default(),
                        },
                        win,
                        cx,
                    );
                    app.dragging_border.is_none()
                })
                .unwrap_or(false);
            check(stale_cleared, "取り残しドラッグ状態は非押下移動で畳まれる");

            // 6. cmd-w でフォーカスペインを閉じる
            press(any, cx, "cmd-w");
            wait(cx, 300).await;
            let (pane_count, terminal_count, survivor) = window
                .update(cx, |app, _, _| {
                    (
                        app.workspace.active_tab().tree().len(),
                        app.terminals.len(),
                        app.focused_pane(),
                    )
                })
                .unwrap_or_else(|_| fail("クローズ後の状態取得"));
            check(
                pane_count == 1 && terminal_count == 1,
                "cmd-w で 1 ペインへ",
            );
            check(survivor == pane2, "残ペインへフォーカス引き継ぎ");

            // 7. cmd-t で新タブ
            press(any, cx, "cmd-t");
            wait(cx, 1500).await;
            let (tab_count, is_second_active, terminal_count) = window
                .update(cx, |app, _, _| {
                    let tabs = app.workspace.tabs();
                    (
                        tabs.len(),
                        tabs.last().map(|t| t.id()) == Some(app.workspace.active_tab_id()),
                        app.terminals.len(),
                    )
                })
                .unwrap_or_else(|_| fail("新タブ後の状態取得"));
            check(
                tab_count == 2 && is_second_active && terminal_count == 2,
                "cmd-t で新タブ",
            );

            // 8. 色つき出力（ANSI 赤）が Theme の赤へ解決される
            type_text(any, cx, r"printf '\e[31mTAKO-RED\e[0m\n'", true);
            wait(cx, 1000).await;
            let red_ok = window
                .update(cx, |app, _, _| {
                    let theme = app.theme.clone();
                    app.focused_session()
                        .map(|s| {
                            let screen = s.screen(&theme);
                            screen.lines.iter().any(|line| {
                                line.text.contains("TAKO-RED")
                                    && line.runs.iter().any(|run| {
                                        run.fg == theme.ansi[1]
                                            && line.text[run.range.clone()].contains("TAKO-RED")
                                    })
                            })
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(red_ok, "ANSI 赤の解決");

            // 9. スクロールバック表示
            type_text(any, cx, "seq 1 200", true);
            wait(cx, 1200).await;
            let scroll_ok = window
                .update(cx, |app, _, _| {
                    let theme = app.theme.clone();
                    app.focused_session()
                        .map(|s| {
                            s.scroll_display(5);
                            let screen = s.screen(&theme);
                            let ok = screen.display_offset == 5 && screen.cursor.is_none();
                            s.scroll_to_bottom();
                            ok
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(scroll_ok, "スクロールバック表示");

            // 10. cmd-v ペースト
            let _ = window.update(cx, |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string("TAKO-PASTE-OK".into()));
            });
            press(any, cx, "cmd-v");
            wait(cx, 800).await;
            check(
                focused_contains(window, cx, "TAKO-PASTE-OK"),
                "cmd-v ペースト",
            );

            // 11. 選択 → cmd-c コピー
            let selected = window
                .update(cx, |app, _, _| {
                    let session = app.focused_session()?;
                    let lines = session.visible_lines();
                    let (row, line) = lines
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, l)| l.contains("TAKO-PASTE-OK"))?;
                    let col = line.find("TAKO-PASTE-OK")?;
                    session.start_selection(SelectionKind::Simple, col, row, false);
                    session.extend_selection(col + "TAKO-PASTE-OK".len() - 1, row, true);
                    Some(())
                })
                .ok()
                .flatten();
            check(selected.is_some(), "選択範囲の設定");
            press(any, cx, "cmd-c");
            wait(cx, 200).await;
            let copied = window
                .update(cx, |_, _, cx| {
                    cx.read_from_clipboard().and_then(|item| item.text())
                })
                .ok()
                .flatten();
            check(copied.as_deref() == Some("TAKO-PASTE-OK"), "cmd-c コピー");

            // 12. cmd-1 でタブ切替
            press(any, cx, "cmd-1");
            let back_to_first = window
                .update(cx, |app, _, _| {
                    app.workspace.tabs().first().map(|t| t.id())
                        == Some(app.workspace.active_tab_id())
                })
                .unwrap_or(false);
            check(back_to_first, "cmd-1 でタブ 1 へ");

            // 13. PTY リサイズ追従（初期 80x24 から実寸へ広がっている）
            let resized = window
                .update(cx, |app, _, _| {
                    app.focused_session()
                        .map(|s| s.size().0 > INITIAL_COLS)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(resized, "PTY リサイズ追従");

            // --- Phase 2: 制御プレーン（環境変数注入 + IPC + tako CLI の e2e）---
            // ここからはペイン内のシェルから実際に `tako` CLI を叩いて検証する。
            // 出力マーカーは `$((40+2))` で組み立て、入力エコー行との誤一致を防ぐ

            // 14. CLI バイナリの準備（target/debug/tako。常にビルドして鮮度を保証）
            let (cli_path, cli) = {
                let built = std::process::Command::new("cargo")
                    .args(["build", "-p", "tako-cli", "--quiet"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                let path = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("tako")))
                    .filter(|p| p.exists());
                let Some(path) = path else {
                    fail("tako CLI のビルド / パス特定");
                };
                check(built, "tako CLI のビルド");
                let quoted = format!("\"{}\"", path.display());
                (path, quoted)
            };

            // 現状: タブ 1 = {ペイン 2}（アクティブ・フォーカス中）、タブ 2 = {ペイン 3}
            let (pane2, tab1) = window
                .update(cx, |app, _, _| {
                    (app.focused_pane(), app.workspace.active_tab_id())
                })
                .unwrap_or_else(|_| fail("Phase 2 開始時の状態取得"));

            // 15. TAKO_PANE_ID / TAKO_TAB_ID の注入（FR-2.1.1）
            type_text(any, cx, "echo P=$TAKO_PANE_ID,T=$TAKO_TAB_ID", true);
            wait(cx, 800).await;
            check(
                focused_contains(window, cx, &format!("P={pane2},T={tab1}")),
                "TAKO_PANE_ID / TAKO_TAB_ID 注入",
            );

            // 16. TAKO_SOCKET（ソケットファイル実在）と TAKO_TOKEN の注入
            type_text(
                any,
                cx,
                "test -S \"$TAKO_SOCKET\" && [ -n \"$TAKO_TOKEN\" ] && echo TAKO-SOCK-$((40+2))",
                true,
            );
            wait(cx, 800).await;
            check(
                focused_contains(window, cx, "TAKO-SOCK-42"),
                "TAKO_SOCKET / TAKO_TOKEN 注入",
            );

            // 17. tako list がペイン内シェルから成功する（FR-2.2.4 / FR-2.2.7）
            type_text(
                any,
                cx,
                &format!("{cli} list >/dev/null && echo TAKO-LIST-$((40+2))"),
                true,
            );
            wait(cx, 1000).await;
            check(focused_contains(window, cx, "TAKO-LIST-42"), "tako list");

            // 18. tako split --down（呼び出し元の自動特定 + origin=cli + フォーカス移動）
            type_text(any, cx, &format!("{cli} split --down"), true);
            wait(cx, 1500).await;
            let (pane_count, pane4, origin_cli) = window
                .update(cx, |app, _, _| {
                    let tree = app.workspace.active_tab().tree();
                    let focused = tree.focused();
                    (
                        tree.len(),
                        focused,
                        tree.get(focused).map(|p| p.origin()) == Some(PaneOrigin::Cli),
                    )
                })
                .unwrap_or_else(|_| fail("tako split 後の状態取得"));
            check(pane_count == 2, "tako split で 2 ペイン");
            check(pane4 != pane2, "split 後フォーカスは新ペイン");
            check(origin_cli, "新ペインの origin は cli");

            // 19. tako send で別ペインへコマンドを流し込む（FR-2.2.2。pane4 から pane2 へ）
            type_text(
                any,
                cx,
                &format!("{cli} send --pane {pane2} 'echo TAKO-SEND-$((40+2))'"),
                true,
            );
            wait(cx, 1200).await;
            let sent = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&pane2)
                        .map(|s| s.visible_lines().iter().any(|l| l.contains("TAKO-SEND-42")))
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(sent, "tako send で別ペインへ送信");

            // 20. tako read で別ペインの画面内容を取得する（FR-2.2.5）
            type_text(
                any,
                cx,
                &format!(
                    "{cli} read --pane {pane2} | grep -q TAKO-SEND-42 && echo TAKO-READ-$((40+2))"
                ),
                true,
            );
            wait(cx, 1000).await;
            check(focused_contains(window, cx, "TAKO-READ-42"), "tako read");

            // 21. tako title --role（FR-2.2.6 / FR-2.1.3）
            type_text(
                any,
                cx,
                &format!("{cli} title --pane {pane2} --role worker-1 REVIEWER"),
                true,
            );
            wait(cx, 800).await;
            let titled = window
                .update(cx, |app, _, _| {
                    app.workspace
                        .get_tab(tab1)
                        .and_then(|t| t.tree().get(pane2))
                        .map(|p| p.title() == Some("REVIEWER") && p.role() == Some("worker-1"))
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(titled, "tako title / role 設定");

            // 22. tako resize --share-y（FR-2.5.6。pane2 の縦取り分を 0.7 へ）
            type_text(
                any,
                cx,
                &format!("{cli} resize --pane {pane2} --share-y 0.7"),
                true,
            );
            wait(cx, 800).await;
            let share = window
                .update(cx, |app, _, _| {
                    app.workspace
                        .active_tab()
                        .tree()
                        .layout(Rect::UNIT)
                        .into_iter()
                        .find(|(id, _)| *id == pane2)
                        .map(|(_, r)| r.height)
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0);
            check((share - 0.7).abs() < 0.01, "tako resize");

            // 23. tako equalize（FR-2.5.7。呼び出し元ペインのタブを均等化）
            type_text(any, cx, &format!("{cli} equalize"), true);
            wait(cx, 800).await;
            let share = window
                .update(cx, |app, _, _| {
                    app.workspace
                        .active_tab()
                        .tree()
                        .layout(Rect::UNIT)
                        .into_iter()
                        .find(|(id, _)| *id == pane2)
                        .map(|(_, r)| r.height)
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0);
            check((share - 0.5).abs() < 0.01, "tako equalize");

            // 24. tako focus <id>（FR-2.2.3）
            type_text(any, cx, &format!("{cli} focus {pane2}"), true);
            wait(cx, 800).await;
            let refocused = window
                .update(cx, |app, _, _| app.focused_pane())
                .unwrap_or_else(|_| fail("tako focus 後の状態取得"));
            check(refocused == pane2, "tako focus");

            // 25. tako tab new（FR-2.5.10。pane2 から実行 → 新タブがアクティブに）
            type_text(any, cx, &format!("{cli} tab new --title agents"), true);
            wait(cx, 1500).await;
            let (tab_count, pane5, on_new_tab) = window
                .update(cx, |app, _, _| {
                    let active = app.workspace.active_tab();
                    (
                        app.workspace.tabs().len(),
                        active.tree().focused(),
                        active.title() == "agents",
                    )
                })
                .unwrap_or_else(|_| fail("tako tab new 後の状態取得"));
            check(tab_count == 3 && on_new_tab, "tako tab new");

            // 26. tako tab move-pane（呼び出し元 pane5 をタブ 1 へ移送。元タブは消える）
            type_text(any, cx, &format!("{cli} tab move-pane {tab1}"), true);
            wait(cx, 1000).await;
            let moved = window
                .update(cx, |app, _, _| {
                    app.workspace.tabs().len() == 2
                        && app.workspace.find_tab_of_pane(pane5) == Some(tab1)
                })
                .unwrap_or(false);
            check(moved, "tako tab move-pane");

            // 27. tako tab select（アクティブタブを 1 へ戻す）。
            // 入力先のペイン 3 にはステップ 10 のペースト残留があるため ctrl-u で行を消す
            press(any, cx, "ctrl-u");
            type_text(any, cx, &format!("{cli} tab select {tab1}"), true);
            wait(cx, 800).await;
            let selected = window
                .update(cx, |app, _, _| app.workspace.active_tab_id() == tab1)
                .unwrap_or(false);
            check(selected, "tako tab select");

            // 28. tako close --pane（FR-2.5.4。pane4 を片付ける）
            type_text(any, cx, &format!("{cli} close --pane {pane4}"), true);
            wait(cx, 1000).await;
            let closed = window
                .update(cx, |app, _, _| {
                    !app.workspace
                        .get_tab(tab1)
                        .map(|t| t.tree().contains(pane4))
                        .unwrap_or(true)
                        && !app.terminals.contains_key(&pane4)
                })
                .unwrap_or(false);
            check(closed, "tako close");

            // 29. 不正トークンの接続拒否（FR-2.3.4。直接ソケットへ書き込んで確認）。
            // UnixStream を使うため unix 限定（Windows の IPC は Phase 6 で実装）
            #[cfg(unix)]
            {
                let endpoint = window
                    .update(cx, |app, _, _| {
                        app.ipc.as_ref().map(|ipc| ipc.endpoint().to_string())
                    })
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| fail("IPC エンドポイント取得"));
                let auth_rejected = (|| -> Option<bool> {
                    use std::io::{BufRead, BufReader, Write};
                    let stream = std::os::unix::net::UnixStream::connect(&endpoint).ok()?;
                    let mut writer = stream.try_clone().ok()?;
                    writeln!(
                        writer,
                        r#"{{"jsonrpc":"2.0","id":1,"token":"bogus","method":"list"}}"#
                    )
                    .ok()?;
                    let mut line = String::new();
                    BufReader::new(stream).read_line(&mut line).ok()?;
                    Some(line.contains("-32001"))
                })()
                .unwrap_or(false);
                check(auth_rejected, "不正トークンの拒否");
            }

            // --- Phase 3: 内蔵 MCP サーバー（Streamable HTTP + stdio ブリッジ）---

            // 30. TAKO_MCP_URL の注入（FR-2.3.2）
            press(any, cx, "ctrl-u");
            type_text(
                any,
                cx,
                "[ -n \"$TAKO_MCP_URL\" ] && echo TAKO-MCPENV-$((40+2))",
                true,
            );
            wait(cx, 800).await;
            check(
                focused_contains(window, cx, "TAKO-MCPENV-42"),
                "TAKO_MCP_URL 注入",
            );

            // MCP / IPC の接続情報と呼び出し元ペインを取得
            let (mcp_url, token, ipc_endpoint, caller) = window
                .update(cx, |app, _, _| {
                    (
                        app.mcp.as_ref().map(|m| m.url().to_string()),
                        app.token.clone(),
                        app.ipc.as_ref().map(|i| i.endpoint().to_string()),
                        app.focused_pane(),
                    )
                })
                .unwrap_or_else(|_| fail("MCP 接続情報の取得"));
            let Some(mcp_url) = mcp_url else {
                fail("MCP サーバーの起動");
            };
            let Some(token) = token else {
                fail("セッショントークンの生成");
            };
            let Some(ipc_endpoint) = ipc_endpoint else {
                fail("IPC エンドポイントの取得");
            };
            let caller_str = caller.to_string();
            const INIT_MSG: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"self-test","version":"0"}}}"#;
            const INITIALIZED_MSG: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
            const TOOLS_LIST_MSG: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
            const LIST_CALL_MSG: &str = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tako_list_panes","arguments":{}}}"#;

            // 31. MCP initialize ハンドシェイク + initialized 通知（FR-2.3.1）
            let (status, response) = mcp_post_bg(cx, &mcp_url, Some(&token), &[], INIT_MSG)
                .await
                .unwrap_or_else(|| fail("MCP initialize 接続"));
            check(
                status == 200
                    && response.contains(r#""serverInfo""#)
                    && response.contains("tako"),
                "MCP initialize",
            );
            let (status, _) = mcp_post_bg(cx, &mcp_url, Some(&token), &[], INITIALIZED_MSG)
                .await
                .unwrap_or_else(|| fail("MCP initialized 通知"));
            check(status == 202, "MCP 通知は 202");

            // 32. tools/list が FR-2.5 の操作セットを公開している
            let (status, response) = mcp_post_bg(cx, &mcp_url, Some(&token), &[], TOOLS_LIST_MSG)
                .await
                .unwrap_or_else(|| fail("MCP tools/list 接続"));
            let tool_count = serde_json::from_str::<serde_json::Value>(&response)
                .ok()
                .and_then(|v| v["result"]["tools"].as_array().map(|t| t.len()))
                .unwrap_or(0);
            check(status == 200 && tool_count == 21, "MCP tools/list は 21 ツール");

            // 33. tools/call tako_list_panes（構造化読み取り。FR-2.5.1）
            let (status, response) = mcp_post_bg(cx, &mcp_url, Some(&token), &[], LIST_CALL_MSG)
                .await
                .unwrap_or_else(|| fail("MCP list_panes 接続"));
            check(
                status == 200
                    && response.contains("tabs")
                    && response.contains(r#""isError":false"#),
                "MCP list_panes",
            );

            // 34. tools/call tako_split_pane（X-Tako-Pane で呼び出し元特定 + origin=mcp）と
            //     tako_close_pane での片付け（FR-2.3.3 / FR-2.5.3〜4）
            let (status, response) = mcp_post_bg(
                cx,
                &mcp_url,
                Some(&token),
                &[("X-Tako-Pane", &caller_str)],
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tako_split_pane","arguments":{"direction":"down"}}}"#,
            )
            .await
            .unwrap_or_else(|| fail("MCP split 接続"));
            check(status == 200, "MCP split 応答");
            let new_pane = serde_json::from_str::<serde_json::Value>(&response)
                .ok()
                .and_then(|v| {
                    v["result"]["content"][0]["text"]
                        .as_str()
                        .map(str::to_string)
                })
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|v| v["pane"].as_u64())
                .unwrap_or_else(|| fail("MCP split の応答解釈"));
            wait(cx, 1200).await;
            let origin_mcp = window
                .update(cx, |app, _, _| {
                    app.workspace
                        .active_tab()
                        .tree()
                        .panes()
                        .iter()
                        .find(|p| p.id().as_u64() == new_pane)
                        .map(|p| p.origin())
                        == Some(PaneOrigin::Mcp)
                })
                .unwrap_or(false);
            check(origin_mcp, "MCP split（origin=mcp）");
            let close_msg = format!(
                r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"tako_close_pane","arguments":{{"pane":{new_pane}}}}}}}"#
            );
            let (status, _) = mcp_post_bg(cx, &mcp_url, Some(&token), &[], &close_msg)
                .await
                .unwrap_or_else(|| fail("MCP close 接続"));
            wait(cx, 500).await;
            let closed = window
                .update(cx, |app, _, _| {
                    !app.workspace
                        .active_tab()
                        .tree()
                        .panes()
                        .iter()
                        .any(|p| p.id().as_u64() == new_pane)
                })
                .unwrap_or(false);
            check(status == 200 && closed, "MCP close_pane で片付け");

            // 35. 不正トークン / 不正 Origin の拒否（FR-2.3.4）
            let (status, _) = mcp_post_bg(cx, &mcp_url, Some("bogus"), &[], LIST_CALL_MSG)
                .await
                .unwrap_or_else(|| fail("MCP 不正トークン接続"));
            check(status == 401, "MCP 不正トークンは 401");
            let (status, _) = mcp_post_bg(
                cx,
                &mcp_url,
                Some(&token),
                &[("Origin", "http://evil.example")],
                LIST_CALL_MSG,
            )
            .await
            .unwrap_or_else(|| fail("MCP 不正 Origin 接続"));
            check(status == 403, "MCP 不正 Origin は 403");

            // 36. stdio ブリッジ（`tako mcp serve`）e2e: 環境変数から接続情報を読み、
            //     IPC へ中継して list が通る（Claude Code 連携と同じ経路）
            let lines = bridge_roundtrip_bg(
                cx,
                cli_path.clone(),
                vec![
                    ("TAKO_SOCKET".into(), ipc_endpoint.clone()),
                    ("TAKO_TOKEN".into(), token.clone()),
                    ("TAKO_PANE_ID".into(), caller_str.clone()),
                ],
                &[INIT_MSG, INITIALIZED_MSG, LIST_CALL_MSG],
            )
            .await;
            check(
                lines.len() == 2
                    && lines[0].contains(r#""serverInfo""#)
                    && lines[1].contains("tabs")
                    && lines[1].contains(r#""isError":false"#),
                "stdio ブリッジ e2e",
            );
            // tako の外では 0 ツール（user スコープ登録済みでも他セッションを邪魔しない）
            let lines =
                bridge_roundtrip_bg(cx, cli_path.clone(), Vec::new(), &[INIT_MSG, TOOLS_LIST_MSG])
                    .await;
            check(
                lines.len() == 2 && lines[1].contains(r#""tools":[]"#),
                "ブリッジは tako 外で 0 ツール",
            );

            // --- Phase 3.5: IME 変換状態（FR-1.9）---
            // NSTextInputClient 経由の実イベントは合成できないため、EntityInputHandler の
            // 実装メソッドを直接呼んで状態遷移と PTY への流れを機械検証する。
            // 変換中テキストの見た目は手動チェック（.agent/manual-checks.md）

            // 37. 未確定文字列（marked text）は状態として保持され、PTY へは流れない
            press(any, cx, "ctrl-u");
            let marked_ok = window
                .update(cx, |app, window, cx| {
                    app.replace_and_mark_text_in_range(None, "にほんご", Some(0..4), window, cx);
                    let bounds = app.bounds_for_range(
                        0..4,
                        Bounds::new(point(px(0.0), px(0.0)), size(px(0.0), px(0.0))),
                        window,
                        cx,
                    );
                    // 文書全体基準のオフセット（ライブ変換）は文節先頭と同じ位置に出る
                    // （回帰: 打ち進めるほど候補ウィンドウが右へ離れる）
                    let bounds_oob = app.bounds_for_range(
                        1000..1004,
                        Bounds::new(point(px(0.0), px(0.0)), size(px(0.0), px(0.0))),
                        window,
                        cx,
                    );
                    // "にほんご" は UTF-16 で 4 コード単位
                    app.marked_text_range(window, cx) == Some(0..4)
                        && app.ime.as_ref().map(|i| i.text.as_str()) == Some("にほんご")
                        && bounds.is_some()
                        && bounds_oob.map(|b| b.origin) == bounds.map(|b| b.origin)
                })
                .unwrap_or(false);
            check(marked_ok, "IME marked text の保持と位置出し");
            wait(cx, 600).await;
            check(
                !focused_contains(window, cx, "にほんご"),
                "IME 変換中は PTY へ流れない",
            );

            // 38. 確定（insertText 相当）で PTY へ書かれ、変換状態が畳まれる
            let committed = window
                .update(cx, |app, window, cx| {
                    app.replace_text_in_range(None, "echo IME-$((40+2))-にほんご", window, cx);
                    app.ime.is_none()
                })
                .unwrap_or(false);
            check(committed, "IME 確定で変換状態クリア");
            press(any, cx, "enter");
            wait(cx, 1000).await;
            check(
                focused_contains(window, cx, "IME-42-にほんご"),
                "IME 確定文字列が PTY へ",
            );

            // 39. unmarkText は「未確定文字列をそのまま挿入」（NSTextInputClient の規約）
            press(any, cx, "ctrl-u");
            let unmarked = window
                .update(cx, |app, window, cx| {
                    app.replace_and_mark_text_in_range(None, "かくてい", None, window, cx);
                    app.unmark_text(window, cx);
                    app.ime.is_none()
                })
                .unwrap_or(false);
            check(unmarked, "unmark で変換状態クリア");
            wait(cx, 800).await;
            check(
                focused_contains(window, cx, "かくてい"),
                "unmark はそのまま挿入",
            );

            // 40. 【回帰】2 ペイン構成（左右分割）で非フォーカス側を CLI close しても落ちない
            //     （2026-06-11 常用報告: 根分割の崩しで panic→SIGABRT）。
            //     直前の IME テストが入力行に残した「かくてい」を ctrl-u で消してから打つ
            press(any, cx, "ctrl-u");
            type_text(any, cx, &format!("{cli} tab new --title close-reg"), true);
            wait(cx, 1200).await;
            let reg_pane_a = window
                .update(cx, |app, _, _| app.workspace.active_tab().tree().focused())
                .unwrap_or_else(|_| fail("回帰 40: タブ作成後の状態取得"));
            type_text(any, cx, &format!("{cli} split --right"), true);
            wait(cx, 1500).await;
            let reg_pane_b = window
                .update(cx, |app, _, _| app.workspace.active_tab().tree().focused())
                .unwrap_or_else(|_| fail("回帰 40: split 後の状態取得"));
            check(reg_pane_b != reg_pane_a, "回帰 40: split で新ペイン");
            // 旧ペインへフォーカスを戻し、新ペイン（非フォーカス側）を外から閉じる
            type_text(any, cx, &format!("{cli} focus {reg_pane_a}"), true);
            wait(cx, 800).await;
            type_text(any, cx, &format!("{cli} close --pane {reg_pane_b}"), true);
            wait(cx, 1500).await;
            let collapsed = window
                .update(cx, |app, _, _| {
                    let tree = app.workspace.active_tab().tree();
                    tree.len() == 1
                        && tree.focused() == reg_pane_a
                        && !app.terminals.contains_key(&reg_pane_b)
                })
                .unwrap_or(false);
            check(collapsed, "CLI close 非フォーカスペインで根分割が崩れる");

            // 40b. split→close を 10 周しても落ちず fd が漏れない（PTY 起動は fd を食うため、
            //      リークすると日常使用で fd 枯渇 → spawn 失敗に至る）
            let fd_before = std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0);
            type_text(
                any,
                cx,
                &format!(
                    "for i in $(seq 1 10); do p=$({cli} split --right) && {cli} close --pane $p; done; echo TAKO-STRESS-$((40+2))"
                ),
                true,
            );
            wait(cx, 10000).await;
            check(
                focused_contains(window, cx, "TAKO-STRESS-42"),
                "split/close ストレス 10 周",
            );
            let stress_stable = window
                .update(cx, |app, _, _| {
                    app.workspace.active_tab().tree().len() == 1
                        && app.workspace.active_tab().tree().focused() == reg_pane_a
                })
                .unwrap_or(false);
            check(stress_stable, "ストレス後もツリーが安定");
            let fd_after = std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0);
            check(
                fd_after <= fd_before + 8,
                "split/close で fd が漏れない",
            );

            // 片付け（最後の 1 ペイン close = タブごと閉じる経路も通す）
            type_text(any, cx, &format!("{cli} close"), true);
            wait(cx, 1000).await;

            // 41. シェル統合（zsh 自動注入）→ OSC 7 / 133 タップ → cwd / state が反映され
            //     list で公開される（FR-2.4.1 + FR-2.1.4 の e2e。実コマンドで検証する）
            press(any, cx, "ctrl-u");
            type_text(any, cx, "cd /private/tmp", true);
            wait(cx, 1000).await;
            let osc_cwd_ok = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&app.focused_pane())
                        .and_then(|s| s.cwd())
                        .map(|p| p == std::path::Path::new("/private/tmp"))
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(osc_cwd_ok, "シェル統合の OSC 7 で cwd 検知");
            type_text(any, cx, "sleep 2", true);
            wait(cx, 700).await;
            let osc_running = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&app.focused_pane())
                        .map(|s| s.command_state() == CommandState::Running)
                        == Some(true)
                })
                .unwrap_or(false);
            check(osc_running, "実行中コマンドが running");
            wait(cx, 1800).await; // sleep 2 の完了を待つ
            type_text(any, cx, "false", true);
            wait(cx, 800).await;
            let osc_failed = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&app.focused_pane())
                        .map(|s| s.command_state() == CommandState::Failed(1))
                        == Some(true)
                })
                .unwrap_or(false);
            check(osc_failed, "失敗コマンドで failed（プロンプト後も保持）");
            // 開発不変条件: 検知した状態は list（CLI / MCP 共有の dispatch）からも見える
            let list_exposes = window
                .update(cx, |app, _, _| {
                    let focused = app.focused_pane().as_u64();
                    let value = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::List,
                        PaneOrigin::Cli,
                    )
                    .expect("list は常に成功する");
                    value["tabs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|t| t["panes"].as_array().into_iter().flatten())
                        .any(|p| {
                            p["id"].as_u64() == Some(focused)
                                && p["state"].as_str() == Some("failed")
                                && p["exit_code"].as_i64() == Some(1)
                                && p["cwd"].as_str() == Some("/private/tmp")
                        })
                })
                .unwrap_or(false);
            check(list_exposes, "list が state / exit_code / cwd を公開");

            // 41b. split が分割元の cwd を継承する（OSC 7 連携。FR-2.4.1）
            type_text(any, cx, &format!("{cli} split --right"), true);
            wait(cx, 2000).await;
            let inherited = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&app.focused_pane())
                        .and_then(|s| s.cwd())
                        .map(|p| p == std::path::Path::new("/private/tmp"))
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(inherited, "split が分割元の cwd を継承");
            // 片付け: 新ペインを閉じ、状態を idle へ戻す
            type_text(any, cx, &format!("{cli} close"), true);
            wait(cx, 800).await;
            type_text(any, cx, "true", true);
            wait(cx, 500).await;

            // 42. 接続情報の永続化と発見（FR-2.2.9）: 環境変数なしでもファイル発見で
            //     CLI が繋がる（アプリ再起動後に外部の長寿命プロセスが繋ぎ直す経路と同じ）
            type_text(
                any,
                cx,
                &format!(
                    "env -u TAKO_SOCKET -u TAKO_TOKEN -u TAKO_PANE_ID {cli} list >/dev/null \
                     && echo TAKO-DISC-$((40+2))"
                ),
                true,
            );
            wait(cx, 1200).await;
            check(
                focused_contains(window, cx, "TAKO-DISC-42"),
                "環境変数なしでファイル発見接続",
            );
            // 42b. 古い環境変数からのフォールバック（接続不可 → ファイル、認証失敗 → ファイル）
            type_text(
                any,
                cx,
                &format!(
                    "TAKO_SOCKET=/nonexistent.sock {cli} list >/dev/null \
                     && TAKO_TOKEN=bogus-stale-token {cli} list >/dev/null \
                     && echo TAKO-STALE-$((40+2))"
                ),
                true,
            );
            wait(cx, 1200).await;
            check(
                focused_contains(window, cx, "TAKO-STALE-42"),
                "古い環境変数からフォールバック",
            );

            // 43. ホイールの出し分け: 通常画面 = 自前スクロールバック表示、
            //     alternate screen = PTY 転送（TUI が自前処理。回帰: Claude Code TUI で
            //     チャットを遡れない）。転送バイト列の網羅はユニットテスト側
            let wheel_up = |app: &mut TakoApp, win: &mut Window, cx: &mut Context<TakoApp>| {
                let pane = app.focused_pane();
                let center = app
                    .pane_text_areas
                    .iter()
                    .find(|(id, _)| *id == pane)
                    .map(|(_, b)| b.center())
                    .unwrap_or_default();
                app.on_pane_scroll(
                    pane,
                    &ScrollWheelEvent {
                        position: center,
                        delta: ScrollDelta::Lines(point(0.0, 2.0)),
                        ..ScrollWheelEvent::default()
                    },
                    win,
                    cx,
                );
            };
            type_text(any, cx, "seq 200", true);
            wait(cx, 1000).await;
            let scrolled_normal = window
                .update(cx, |app, win, cx| {
                    wheel_up(app, win, cx);
                    let offset = app
                        .terminals
                        .get(&app.focused_pane())
                        .map(|s| s.display_offset())
                        .unwrap_or(0);
                    if let Some(s) = app.terminals.get(&app.focused_pane()) {
                        s.scroll_to_bottom();
                    }
                    offset > 0
                })
                .unwrap_or(false);
            check(scrolled_normal, "通常画面のホイールでスクロールバック");
            // alternate screen 中は自前スクロールせず PTY へ転送される
            type_text(any, cx, r"printf '\e[?1049h'; sleep 2; printf '\e[?1049l'", true);
            wait(cx, 800).await;
            let forwarded_in_alt = window
                .update(cx, |app, win, cx| {
                    wheel_up(app, win, cx);
                    app.terminals
                        .get(&app.focused_pane())
                        .map(|s| s.display_offset())
                        == Some(0)
                })
                .unwrap_or(false);
            check(forwarded_in_alt, "alt screen のホイールは PTY 転送");
            wait(cx, 2000).await; // alt screen 解除を待つ

            // 44. スクロール操作の CLI 公開（FR-2.5.13）とスクロールバードラッグ換算。
            //     43 で alt screen へ転送した矢印キーが復帰後の zle に届き履歴を遡っているため、
            //     ctrl-u で行をクリアしてから打つ。タイプ（write）は最下部へ戻し、スクロール中は
            //     新しい出力が画面外になるため、成否は echo ではなく offset で検証する
            press(any, cx, "ctrl-u");
            type_text(any, cx, &format!("{cli} scroll --to 5 >/dev/null"), true);
            wait(cx, 1000).await;
            let cli_scrolled = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&app.focused_pane())
                        .map(|s| s.display_offset() >= 5)
                        == Some(true)
                })
                .unwrap_or(false);
            check(cli_scrolled, "tako scroll --to が表示位置に反映");
            let (drag_top_ok, drag_bottom_ok, drag_cleared) = window
                .update(cx, |app, _, cx| {
                    let pane = app.focused_pane();
                    let area = app
                        .pane_text_areas
                        .iter()
                        .find(|(id, _)| *id == pane)
                        .map(|(_, b)| *b)
                        .expect("フォーカスペインのレイアウトはある");
                    // トラック上端へドラッグ = 最古、下端 = 最下部（サム中心合わせ + クランプ）
                    app.start_scrollbar_drag(pane, area.origin.y, cx);
                    let top_ok = app
                        .terminals
                        .get(&pane)
                        .map(|s| s.display_offset() == s.history_size())
                        == Some(true);
                    app.scrollbar_drag_to(pane, area.origin.y + area.size.height, cx);
                    let bottom_ok = app
                        .terminals
                        .get(&pane)
                        .map(|s| s.display_offset() == 0)
                        == Some(true);
                    app.on_mouse_up(
                        &MouseUpEvent {
                            button: MouseButton::Left,
                            position: point(px(0.0), px(0.0)),
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        },
                        cx,
                    );
                    (top_ok, bottom_ok, app.dragging_scrollbar.is_none())
                })
                .unwrap_or((false, false, false));
            check(drag_top_ok, "スクロールバードラッグで最上部へ");
            check(
                drag_bottom_ok && drag_cleared,
                "スクロールバードラッグで最下部へ（解放でクリア）",
            );

            // 45. kitty keyboard protocol（disambiguate）の有効化を検知する
            //     （回帰: Claude Code TUI で Shift+Enter 改行が効かない。
            //     CSI u へのバイト変換はユニットテスト側で網羅）
            type_text(any, cx, r"printf '\e[>1u'; sleep 1; printf '\e[<u'", true);
            wait(cx, 500).await;
            let disambiguate_on = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&app.focused_pane())
                        .map(|s| s.disambiguate_keys())
                        == Some(true)
                })
                .unwrap_or(false);
            check(disambiguate_on, "kitty protocol push で disambiguate 検知");
            wait(cx, 1200).await;
            let disambiguate_off = window
                .update(cx, |app, _, _| {
                    app.terminals
                        .get(&app.focused_pane())
                        .map(|s| s.disambiguate_keys())
                        == Some(false)
                })
                .unwrap_or(false);
            check(disambiguate_off, "kitty protocol pop で解除");

            // 46. 全角行のマウス座標→セル変換（回帰: 範囲選択が見た目よりだいぶ左から始まる）。
            //     全角の描画幅はセル幅 × 2 と一致しないため、描画上の「う」の位置をクリック
            //     したとき、グリッド col = 4（全角 2 セル × 2 文字ぶん）に解決されること
            press(any, cx, "ctrl-u");
            type_text(any, cx, "echo あいうえおかきくけこ", true);
            wait(cx, 1000).await;
            let wide_hit = window
                .update(cx, |app, win, _| {
                    let pane = app.focused_pane();
                    let (_, area) = app
                        .pane_text_areas
                        .iter()
                        .find(|(id, _)| *id == pane)
                        .copied()?;
                    let cell = app.cell_size?;
                    let screen = app.terminals.get(&pane)?.screen(&app.theme);
                    let row = screen
                        .lines
                        .iter()
                        .position(|l| l.text.starts_with("あいうえおかきくけこ"))?;
                    let line = &screen.lines[row];
                    // 描画と同じ shaping で「う」の先頭 x を求め、少し右をクリックする
                    let shaped = win.text_system().shape_line(
                        SharedString::from(line.text.clone()),
                        px(app.theme.font_size),
                        &[app.mono_text_run(line.text.len())],
                        None,
                    );
                    let x = f32::from(shaped.x_for_index("あい".len())) + 2.0;
                    let pos = point(
                        area.origin.x + px(x),
                        area.origin.y + cell.height * row as f32 + px(2.0),
                    );
                    let (col, hit_row, _) = app.cell_at(pane, pos, win)?;
                    Some(col == 4 && hit_row == row)
                })
                .ok()
                .flatten()
                .unwrap_or(false);
            check(wide_hit, "全角行のクリックが正しいセルに解決");

            // 47. ペインの × ボタン（dispatch 共有経路）。split で増やして × 相当の操作で
            //     片付き、フォーカスが残存ペインへ戻ること
            type_text(any, cx, &format!("{cli} split --right >/dev/null"), true);
            wait(cx, 1500).await;
            let close_button_ok = window
                .update(cx, |app, _, cx| {
                    let before = app.workspace.active_tab().tree().len();
                    let target = app.focused_pane();
                    app.close_pane_button(target, cx);
                    let tree = app.workspace.active_tab().tree();
                    before == 2
                        && tree.len() == 1
                        && !tree.contains(target)
                        && !app.terminals.contains_key(&target)
                })
                .unwrap_or(false);
            check(close_button_ok, "ペインの × ボタンで閉じる（dispatch 経由）");

            // 48. tmux 一覧と kill（FR-2.13）。専用 -L ソケットで隔離し、ユーザーの
            //     実 tmux サーバーには一切触れない。tmux 不在環境ではスキップする
            let has_tmux = std::process::Command::new("tmux")
                .arg("-V")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if has_tmux {
                let sock = format!("tako-selftest-{}", std::process::id());
                let created = std::process::Command::new("tmux")
                    .args(["-L", &sock, "new-session", "-d", "-s", "tako-test"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                check(created, "テスト用 tmux セッション作成");
                wait(cx, 500).await;
                press(any, cx, "ctrl-u");
                type_text(
                    any,
                    cx,
                    &format!(
                        "{cli} tmux list --socket {sock} | grep -q tako-test \
                         && echo TAKO-TMUX-$((40+8))"
                    ),
                    true,
                );
                wait(cx, 1500).await;
                check(focused_contains(window, cx, "TAKO-TMUX-48"), "tako tmux list");
                type_text(
                    any,
                    cx,
                    &format!("{cli} tmux kill --session tako-test --socket {sock}"),
                    true,
                );
                wait(cx, 1200).await;
                let gone = window
                    .update(cx, |app, _, _| {
                        let value = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::TmuxList {
                                socket: Some(sock.clone()),
                            },
                            PaneOrigin::Cli,
                        )
                        .expect("tmux list は常に成功する");
                        value["sessions"].as_array().map(Vec::is_empty) == Some(true)
                    })
                    .unwrap_or(false);
                check(gone, "tako tmux kill でセッションが消える");
                let _ = std::process::Command::new("tmux")
                    .args(["-L", &sock, "kill-server"])
                    .status();
            } else {
                eprintln!("（tmux 不在のため項目 48 をスキップ）");
            }

            // 49. 情報パネルの統合 tmux ビュー（FR-2.13 / FR-2.16.6。dispatch の Panel 操作で
            //     表示 → 一覧更新 → kill の確認フローが畳めること → 非表示）。固定タブ 0 個方針
            let view_ok = window
                .update(cx, |app, _, cx| {
                    let opened = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Panel {
                            visible: Some(true),
                            width: Some(320.0),
                            view: Some(tako_control::protocol::PanelViewWire::Tmux),
                            filetree: None,
                        },
                        PaneOrigin::Cli,
                    );
                    let opened_ok = matches!(&opened, Ok(v) if v["visible"].as_bool() == Some(true)
                        && v["view"].as_str() == Some("tmux"));
                    let shown = app.panel_visible && app.panel_view == PanelView::Tmux;
                    // 確認フロー: 存在しないセッションの kill は無害に失敗し pending が畳まれる
                    app.tmux_pending_kill = Some(("tako-no-such-session".into(), None, None));
                    app.tmux_kill_confirmed(cx);
                    let pending_cleared = app.tmux_pending_kill.is_none();
                    let closed = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Panel {
                            visible: Some(false),
                            width: None,
                            view: None,
                            filetree: None,
                        },
                        PaneOrigin::Cli,
                    );
                    let closed_ok = matches!(&closed, Ok(v) if v["visible"].as_bool() == Some(false));
                    opened_ok && shown && pending_cleared && closed_ok && !app.panel_visible
                })
                .unwrap_or(false);
            check(view_ok, "情報パネル（統合 tmux ビュー）の表示・確認フロー・非表示");

            // 50. tako tab rename（FR-2.12.1）: 呼び出し元ペインからタブを解決し、
            //     明示リネームは手動扱い（title_source = manual）でタブ表示名になる
            let (active_tab, active_pane) = window
                .update(cx, |app, _, _| {
                    (app.workspace.active_tab_id(), app.focused_pane())
                })
                .unwrap_or_else(|_| fail("FR-2.12 開始時の状態取得"));
            press(any, cx, "ctrl-u");
            type_text(any, cx, &format!("{cli} tab rename 実験タブ"), true);
            wait(cx, 1000).await;
            let renamed = window
                .update(cx, |app, _, _| {
                    app.workspace
                        .get_tab(active_tab)
                        .map(|t| {
                            t.title() == "実験タブ" && t.title_source() == TitleSource::Manual
                        })
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            check(renamed, "tako tab rename（手動扱い）");

            // 51. 自動リネームの適用と手動優先（FR-2.12.3）: 手動のタブ・ペインは
            //     上書きされず、手動指定の解除後は同じ plan が反映される
            let apply_ok = window
                .update(cx, |app, _, cx| {
                    let tab_u = active_tab.as_u64();
                    let tab = app.workspace.get_tab_mut(active_tab).expect("タブはある");
                    if let Some(pane) = tab.tree_mut().get_mut(active_pane) {
                        pane.set_title(Some("手動名".into()));
                    }
                    let plan = autorename::RenamePlan {
                        tab: Some("自動タブ".into()),
                        panes: vec![(active_pane.as_u64(), "自動ペイン".into())],
                    };
                    app.apply_rename_plan(tab_u, &plan, cx);
                    let tab = app.workspace.get_tab(active_tab).expect("タブはある");
                    let kept = tab.title() == "実験タブ"
                        && tab.tree().get(active_pane).and_then(|p| p.title()) == Some("手動名");
                    // 手動指定を解除すると自動が効くようになる
                    let tab = app.workspace.get_tab_mut(active_tab).expect("タブはある");
                    tab.clear_manual_title();
                    if let Some(pane) = tab.tree_mut().get_mut(active_pane) {
                        pane.set_title(None);
                    }
                    app.apply_rename_plan(tab_u, &plan, cx);
                    let tab = app.workspace.get_tab(active_tab).expect("タブはある");
                    kept && tab.title() == "自動タブ"
                        && tab.title_source() == TitleSource::Auto
                        && tab.tree().get(active_pane).and_then(|p| p.title())
                            == Some("自動ペイン")
                })
                .unwrap_or(false);
            check(apply_ok, "自動リネームの適用と手動優先");

            // 52. tako autorename の ON/OFF と状態取得（FR-2.12.4。CLI / MCP と同じ
            //     dispatch 経路。セルフテスト中は設定ファイルへ永続化しない）
            type_text(
                any,
                cx,
                &format!(
                    "{cli} autorename off >/dev/null && {cli} autorename \
                     | grep -q '\"enabled\":false' && echo TAKO-AR-$((50+2))"
                ),
                true,
            );
            wait(cx, 1200).await;
            check(
                focused_contains(window, cx, "TAKO-AR-52"),
                "tako autorename off / 状態取得",
            );
            let toggled = window
                .update(cx, |app, _, _| !app.autorename.enabled)
                .unwrap_or(false);
            check(toggled, "自動リネームの無効化が検知ループへ反映");

            // 53. listen ポート検知（FR-2.4.2）。ペイン内で nc を listen させ、
            //     tty 突き合わせのポーリング（3 秒毎）が拾うまで待つ。
            //     ポートは bind(0) で空きを取ってから渡す（既知ポートとの衝突回避）
            let free_port = std::net::TcpListener::bind("127.0.0.1:0")
                .ok()
                .and_then(|l| l.local_addr().ok())
                .map(|a| a.port())
                .unwrap_or(12947);
            press(any, cx, "ctrl-u");
            type_text(any, cx, &format!("nc -l {free_port} &"), true);
            let mut detected = false;
            for _ in 0..8 {
                wait(cx, 1500).await;
                detected = window
                    .update(cx, |app, _, _| {
                        app.terminals
                            .get(&app.focused_pane())
                            .map(|s| s.listen_ports().iter().any(|p| p.port == free_port))
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if detected {
                    break;
                }
            }
            check(detected, "listen ポート検知（nc -l）");
            // list（CLI / MCP と同じ dispatch）にも listen_ports として公開される
            let listed = window
                .update(cx, |app, _, _| {
                    let pane = app.focused_pane().as_u64();
                    let value = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::List,
                        PaneOrigin::Cli,
                    )
                    .expect("list は常に成功する");
                    value["tabs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|t| t["panes"].as_array().cloned().unwrap_or_default())
                        .filter(|p| p["id"].as_u64() == Some(pane))
                        .any(|p| {
                            p["listen_ports"]
                                .as_array()
                                .is_some_and(|ports| ports.iter().any(|e| {
                                    e["port"].as_u64() == Some(free_port as u64)
                                }))
                        })
                })
                .unwrap_or(false);
            check(listed, "list に listen_ports が公開される");

            // 54. 提案チップ（FR-2.4.3）: 新規 listen ポートでチップが立ち、却下で消え、
            //     却下中は同じポートを再提案しない
            let chip_up = window
                .update(cx, |app, _, _| {
                    let pane = app.focused_pane();
                    app.port_suggestions
                        .iter()
                        .any(|s| s.pane == pane && s.port == free_port)
                })
                .unwrap_or(false);
            check(chip_up, "listen 検知で提案チップが立つ");
            let chip_dismissed = window
                .update(cx, |app, _, cx| {
                    let pane = app.focused_pane();
                    app.dismiss_port_suggestion(pane, free_port, cx);
                    !app.port_suggestions
                        .iter()
                        .any(|s| s.pane == pane && s.port == free_port)
                        && app.dismissed_ports.contains(&(pane, free_port))
                })
                .unwrap_or(false);
            check(chip_dismissed, "チップの却下と再提案抑止");

            // 55. tako portdetect の ON/OFF（FR-2.4.4。CLI / MCP と同じ dispatch 経路）。
            //     無効化で listen_ports・チップが掃除される
            type_text(
                any,
                cx,
                &format!(
                    "{cli} portdetect off >/dev/null && {cli} portdetect \
                     | grep -q '\"enabled\":false' && echo TAKO-PD-$((50+5))"
                ),
                true,
            );
            wait(cx, 1200).await;
            check(
                focused_contains(window, cx, "TAKO-PD-55"),
                "tako portdetect off / 状態取得",
            );
            let detect_cleared = window
                .update(cx, |app, _, _| {
                    !app.port_detect
                        && app.port_suggestions.is_empty()
                        && app
                            .terminals
                            .values()
                            .all(|s| s.listen_ports().is_empty())
                })
                .unwrap_or(false);
            check(detect_cleared, "ポート検知の無効化で検知済み情報がクリアされる");
            type_text(any, cx, &format!("{cli} portdetect on >/dev/null"), true);
            wait(cx, 800).await;
            type_text(any, cx, "kill %1 2>/dev/null", true);
            wait(cx, 500).await;

            // 56. 統合 tmux ビューのタブ枠データ（FR-2.16.6〜2.16.7。旧集約センター FR-2.10 を
            //     統合）。全タブの全ペインがタブ枠に載り、枠内は注目度順。ジャンプで該当ペインへ
            //     フォーカス（パネルは開いたまま）。ペイン kill の確認フローも畳める
            let groups_ok = window
                .update(cx, |app, _, cx| {
                    let opened = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Panel {
                            visible: Some(true),
                            width: None,
                            view: Some(tako_control::protocol::PanelViewWire::Tmux),
                            filetree: None,
                        },
                        PaneOrigin::Cli,
                    );
                    let opened_ok = matches!(&opened, Ok(v) if v["view"].as_str() == Some("tmux"));
                    let groups = app.tmux_view_groups();
                    // タブ枠がタブと 1:1 + 全ペインが載っている + 枠内は注目度順（単調非減少）
                    let tabs_match = groups.len() == app.workspace.tabs().len();
                    let total: usize = app
                        .workspace
                        .tabs()
                        .iter()
                        .map(|t| t.tree().panes().len())
                        .sum();
                    let listed: usize = groups.iter().map(|g| g.rows.len()).sum();
                    let all_listed = listed == total && total > 0;
                    let sorted = groups.iter().all(|g| {
                        g.rows
                            .windows(2)
                            .all(|w| state_rank(w[0].state) <= state_rank(w[1].state))
                    });
                    // 非アクティブタブのペインへジャンプ → タブも切り替わる
                    let target = groups
                        .iter()
                        .find(|g| g.tab != app.workspace.active_tab_id())
                        .and_then(|g| g.rows.first())
                        .or_else(|| groups.first().and_then(|g| g.rows.first()))
                        .map(|e| e.pane)
                        .expect("エントリは 1 件以上ある");
                    app.jump_to_pane(target, cx);
                    let jumped = app.focused_pane() == target;
                    // ジャンプしてもパネルは開いたまま（畳むのはユーザー / AI の明示操作）
                    let still_open = app.panel_visible;
                    // ペイン kill（FR-2.16.7）: 一時ペインを生やし、ゴミ箱 → 確認 → kill の
                    // 経路（pending → pane_kill_confirmed = dispatch Close）で消えること。
                    // dispatch を直接呼ぶためセッション起動依頼（pending_attach）はここで
                    // 処理する（IPC ループ相当。残すと後続 dispatch が死んだペインを起動する）
                    let split = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Split {
                            pane: Some(target.as_u64()),
                            direction: None,
                            ratio: None,
                            command: None,
                            cwd: None,
                        },
                        PaneOrigin::Cli,
                    );
                    let temp = split
                        .ok()
                        .and_then(|v| v["pane"].as_u64())
                        .expect("split は成功する");
                    for (pane, options) in std::mem::take(&mut app.pending_attach) {
                        app.spawn_session(pane, options, cx)
                            .expect("一時ペインの PTY 起動は成功する");
                    }
                    let temp_id = app
                        .workspace
                        .tabs()
                        .iter()
                        .flat_map(|t| t.tree().panes())
                        .map(|p| p.id())
                        .find(|p| p.as_u64() == temp)
                        .expect("生やしたペインは存在する");
                    app.pending_pane_kill = Some(temp_id);
                    app.pane_kill_confirmed(cx);
                    let killed = app.pending_pane_kill.is_none()
                        && !app
                            .workspace
                            .tabs()
                            .iter()
                            .flat_map(|t| t.tree().panes())
                            .any(|p| p.id() == temp_id);
                    app.panel_visible = false;
                    opened_ok && tabs_match && all_listed && sorted && jumped && still_open && killed
                })
                .unwrap_or(false);
            check(groups_ok, "統合 tmux ビューのタブ枠一覧・ジャンプ・kill 確認フロー");

            // 57. ファイルツリー（FR-3.1 / FR-3.7）。cmd+B で開閉し、表示中は
            //     タブ内ペインの cwd（無ければ $HOME）がワークスペースフォルダとして並ぶ
            press(any, cx, "cmd-b");
            wait(cx, 600).await;
            let sidebar_ok = window
                .update(cx, |app, _, _| {
                    let visible = app.filetree.visible;
                    let root_ok = !app.filetree.roots().is_empty();
                    let has_rows = !app.filetree.rows().is_empty();
                    visible && root_ok && has_rows
                })
                .unwrap_or(false);
            check(sidebar_ok, "ファイルツリーの表示と cwd 追従");
            press(any, cx, "cmd-b");
            wait(cx, 300).await;
            let sidebar_closed = window
                .update(cx, |app, _, _| !app.filetree.visible)
                .unwrap_or(false);
            check(sidebar_closed, "ファイルツリーの折りたたみ（cmd+B）");

            // 58. tako persist の ON/OFF と状態取得（Phase 5.5 / FR-5。CLI / MCP と同じ
            //     dispatch 経路。セルフテスト中は設定・レイアウトを永続化しない）
            type_text(
                any,
                cx,
                &format!(
                    "{cli} persist on >/dev/null && {cli} persist \
                     | grep -q '\"enabled\":true' && echo TAKO-PS-$((50+8))"
                ),
                true,
            );
            wait(cx, 1200).await;
            check(
                focused_contains(window, cx, "TAKO-PS-58"),
                "tako persist on / 状態取得",
            );
            let persist_on = window
                .update(cx, |app, _, _| app.tmux_persist)
                .unwrap_or(false);
            check(persist_on, "persist 有効化が spawn 経路へ反映");

            // 59〜62. tmux バックエンド永続化の実機 e2e（tmux 不在環境ではスキップ）。
            //     隔離ソケット（main() で TAKO_TMUX_SOCKET=tako-st-<pid> を設定済み）上で
            //     セッション生成 → シェル動作 → OSC パススルー → tty 差し替え →
            //     tmuxview 区別 → 明示 close での kill を検証する
            if tako_core::tmux_backend::available() {
                let backend_sock = tako_core::tmux_backend::socket_name();

                // 59. persist 有効中の分割はバックエンドセッションとして生え、シェルが動く
                press(any, cx, "cmd-d");
                wait(cx, 800).await;
                let (backend_pane, backend_name) = window
                    .update(cx, |app, _, _| {
                        let pane = app.focused_pane();
                        (pane, app.backend_sessions.get(&pane).cloned())
                    })
                    .unwrap_or_else(|_| fail("バックエンドペインの状態取得"));
                let backend_name = backend_name.unwrap_or_else(|| {
                    fail("persist 有効中の新ペインにバックエンドセッション名が付く")
                });
                let mut session_up = false;
                for _ in 0..20 {
                    wait(cx, 500).await;
                    session_up = tako_core::tmux::list_sessions(Some(&backend_sock))
                        .iter()
                        .any(|s| s.name == backend_name);
                    if session_up {
                        break;
                    }
                }
                check(session_up, "分割でバックエンドセッションが生える");
                press(any, cx, "ctrl-u");
                type_text(any, cx, "echo TAKO-BK-'OK'", true);
                let mut echoed = false;
                for _ in 0..20 {
                    wait(cx, 500).await;
                    echoed = focused_contains(window, cx, "TAKO-BK-OK");
                    if echoed {
                        break;
                    }
                }
                check(echoed, "バックエンドペインでシェルが動く");

                // 60. OSC 7 パススルー（allow-passthrough + シェル統合の包み直し）で
                //     tmux 越しでも cwd 検知（FR-2.4.1）が生きている
                press(any, cx, "ctrl-u");
                type_text(any, cx, "mkdir -p /tmp/tako-osc-e2e && cd /tmp/tako-osc-e2e", true);
                let mut cwd_ok = false;
                for _ in 0..20 {
                    wait(cx, 500).await;
                    cwd_ok = window
                        .update(cx, |app, _, _| {
                            app.terminals
                                .get(&backend_pane)
                                .and_then(|s| s.cwd())
                                .map(|p| p.display().to_string().contains("tako-osc-e2e"))
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if cwd_ok {
                        break;
                    }
                }
                check(cwd_ok, "OSC 7 が tmux パススルーで届く（cwd 検知維持）");

                // 61. tty がバックエンド側ペイン tty へ差し替わり（ポート検知・tmuxview の
                //     突き合わせ先）、tmux list が backend: true + 対応ペインで区別される
                let mut tty_ok = false;
                for _ in 0..20 {
                    wait(cx, 500).await;
                    let inner = tako_core::tmux_backend::pane_tty(&backend_sock, &backend_name);
                    tty_ok = inner.is_some()
                        && window
                            .update(cx, |app, _, _| {
                                app.terminals
                                    .get(&backend_pane)
                                    .and_then(|s| s.tty_name().map(str::to_string))
                                    == inner
                            })
                            .unwrap_or(false);
                    if tty_ok {
                        break;
                    }
                }
                check(tty_ok, "tty がバックエンドペイン tty へ差し替わる");
                let listed_backend = window
                    .update(cx, |app, _, _| {
                        let value = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::TmuxList { socket: None },
                            PaneOrigin::Cli,
                        )
                        .expect("tmux list は常に成功する");
                        value["sessions"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|s| {
                                s["name"].as_str() == Some(backend_name.as_str())
                                    && s["backend"].as_bool() == Some(true)
                                    && s["backend_pane"].as_u64()
                                        == Some(backend_pane.as_u64())
                            })
                    })
                    .unwrap_or(false);
                check(listed_backend, "tmux list がバックエンドを区別表示する");

                // 61b. バックエンドのホイール = tako 駆動の tmux スクロール
                //（SGR 任せの copy-mode 突入をやめた方式。コアレッシング + 正確な行数）
                press(any, cx, "ctrl-u");
                type_text(any, cx, "seq 200", true);
                wait(cx, 1000).await;
                window
                    .update(cx, |app, win, cx| {
                        let center = app
                            .pane_text_areas
                            .iter()
                            .find(|(id, _)| *id == backend_pane)
                            .map(|(_, b)| b.center())
                            .unwrap_or_default();
                        app.on_pane_scroll(
                            backend_pane,
                            &ScrollWheelEvent {
                                position: center,
                                delta: ScrollDelta::Lines(point(0.0, 4.0)),
                                ..ScrollWheelEvent::default()
                            },
                            win,
                            cx,
                        );
                    })
                    .ok();
                let mut wheel_scrolled = false;
                for _ in 0..20 {
                    wait(cx, 300).await;
                    wheel_scrolled = window
                        .update(cx, |app, _, _| {
                            app.scroll_ctls
                                .get(&backend_pane)
                                .map(|c| c.state.in_mode && c.state.position > 0)
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if wheel_scrolled {
                        break;
                    }
                }
                check(wheel_scrolled, "バックエンドのホイールが tmux スクロールに乗る");

                // 61c. スクロールバーはスクロール中だけ表示され、時間経過でフェードする
                let bar_visible = window
                    .update(cx, |app, _, _| {
                        let area = app
                            .pane_text_areas
                            .iter()
                            .find(|(id, _)| *id == backend_pane)
                            .map(|(_, b)| *b)
                            .unwrap_or_default();
                        app.scrollbar_overlay(backend_pane, area).is_some()
                    })
                    .unwrap_or(false);
                let bar_faded = window
                    .update(cx, |app, _, _| {
                        if let Some(ctl) = app.scroll_ctls.get_mut(&backend_pane) {
                            ctl.last_activity =
                                std::time::Instant::now() - Duration::from_secs(3);
                        }
                        let area = app
                            .pane_text_areas
                            .iter()
                            .find(|(id, _)| *id == backend_pane)
                            .map(|(_, b)| *b)
                            .unwrap_or_default();
                        app.scrollbar_overlay(backend_pane, area).is_none()
                    })
                    .unwrap_or(false);
                check(
                    bar_visible && bar_faded,
                    "スクロールバーはスクロール中だけ表示（フェード）",
                );

                // 61d. スクロール中のキー入力は最下部へ戻してから流れる（iTerm2 流。
                //      copy-mode にキーが飲まれて入力が反映されない症状の回帰検知）
                press(any, cx, "enter");
                let mut key_cancelled = false;
                for _ in 0..20 {
                    wait(cx, 300).await;
                    key_cancelled = window
                        .update(cx, |app, _, _| {
                            app.scroll_ctls
                                .get(&backend_pane)
                                .map(|c| !c.state.in_mode)
                                .unwrap_or(true)
                        })
                        .unwrap_or(false)
                        && tako_core::scroll::scroll_state(
                            &tako_core::scroll::ScrollTarget::Backend {
                                socket: backend_sock.clone(),
                                session: backend_name.clone(),
                            },
                        )
                        .map(|s| !s.in_mode)
                        .unwrap_or(false);
                    if key_cancelled {
                        break;
                    }
                }
                check(key_cancelled, "スクロール中のキー入力で最下部へ戻る（iTerm2 流）");

                // 61e. CLI（dispatch 共有）でもバックエンドの tmux スクロールに効く
                press(any, cx, "ctrl-u");
                type_text(any, cx, &format!("{cli} scroll --to 5 >/dev/null"), true);
                let mut cli_scrolled = false;
                for _ in 0..20 {
                    wait(cx, 300).await;
                    cli_scrolled = tako_core::scroll::scroll_state(
                        &tako_core::scroll::ScrollTarget::Backend {
                            socket: backend_sock.clone(),
                            session: backend_name.clone(),
                        },
                    )
                    .map(|s| s.position >= 5)
                    .unwrap_or(false);
                    if cli_scrolled {
                        break;
                    }
                }
                check(cli_scrolled, "tako scroll がバックエンドの tmux スクロールに効く");

                // 61f. タブ内ペインで attach 中の外部 tmux セッションはタブ枠へ紐付き、
                //      「管理外 / kill 漏れ?」に出ない（FR-2.16.9。別サーバーのセッションを
                //      tako ペインで attach して見ている構成が管理外扱いされた
                //      2026-06-13 実機バグの回帰検知）
                let tmux_bin = tako_core::tmux::tmux_bin();
                press(any, cx, "ctrl-u");
                type_text(
                    any,
                    cx,
                    &format!(
                        "{tmux_bin} -L {backend_sock} new-session -d -s att-e2e && \
                         {tmux_bin} -L {backend_sock} attach -t att-e2e"
                    ),
                    true,
                );
                let mut attached_ok = false;
                for _ in 0..20 {
                    wait(cx, 500).await;
                    attached_ok = window
                        .update(cx, |app, _, _| {
                            app.refresh_tmux_data();
                            let groups = app.tmux_view_groups();
                            // attach 先ペイン（backend_pane）が居るタブ枠に紐付く
                            let in_group = groups.iter().any(|g| {
                                g.rows.iter().any(|r| r.pane == backend_pane)
                                    && g.sessions.iter().any(|s| {
                                        s.name == "att-e2e"
                                            && s.pane == backend_pane.as_u64()
                                            && !s.windows.is_empty()
                                    })
                            });
                            let not_unlisted = !app
                                .tmux_unlisted_sessions()
                                .iter()
                                .any(|s| s.name == "att-e2e");
                            in_group && not_unlisted
                        })
                        .unwrap_or(false);
                    if attached_ok {
                        break;
                    }
                }
                check(
                    attached_ok,
                    "attach 中の外部 tmux セッションがタブ枠へ紐付く（管理外に出ない）",
                );
                // 後片付け: セッション kill（dispatch 経由 = UI の確認後と同じ経路）で
                // 内側の attach クライアントも終了し、ペインはシェルへ戻る
                let att_killed = window
                    .update(cx, |app, _, _| {
                        let result = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::TmuxKill {
                                socket: Some(backend_sock.clone()),
                                session: "att-e2e".into(),
                                window: None,
                            },
                            PaneOrigin::Cli,
                        );
                        result.is_ok()
                    })
                    .unwrap_or(false);
                check(att_killed, "attach 済みセッションの kill（dispatch TmuxKill）");
                wait(cx, 800).await;

                // 62. ペインの明示 close でバックエンドセッションも破棄される
                //     （アプリ終了では破棄されない = 永続化、は core の e2e テストで検証済み）
                press(any, cx, "cmd-w");
                let mut killed = false;
                for _ in 0..20 {
                    wait(cx, 500).await;
                    killed = !tako_core::tmux::list_sessions(Some(&backend_sock))
                        .iter()
                        .any(|s| s.name == backend_name);
                    if killed {
                        break;
                    }
                }
                check(killed, "明示 close でバックエンドセッションが消える");

                // 後片付け: 隔離バックエンドサーバーごと落とす
                tako_core::tmux_backend::kill_server(&backend_sock);
            } else {
                eprintln!("（tmux 不在のため項目 59〜62 をスキップ）");
            }
            // persist を OFF に戻す（以後の項目・終了処理への影響を断つ）
            let persist_off = window
                .update(cx, |app, _, _| {
                    let result = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Persist {
                            enabled: Some(false),
                        },
                        PaneOrigin::Cli,
                    );
                    matches!(result, Ok(v) if v["enabled"].as_bool() == Some(false))
                })
                .unwrap_or(false);
            check(persist_off, "tako persist off（dispatch 経由）");

            // 63. 明示コマンド付き split の回帰（2026-06-12 リグレッション (7)）。
            //     コマンドはログインシェル経由で実行される（最小 PATH の .app でも
            //     `tmux attach` 等が解決できる）。出力マーカーで実行を機械検証する
            press(any, cx, "ctrl-u");
            type_text(
                any,
                cx,
                &format!("{cli} split --down -- sh -c 'echo TAKO-CMD-\"OK\"; sleep 15'"),
                true,
            );
            let mut cmd_ok = false;
            for _ in 0..15 {
                wait(cx, 600).await;
                cmd_ok = focused_contains(window, cx, "TAKO-CMD-OK");
                if cmd_ok {
                    break;
                }
            }
            check(cmd_ok, "明示コマンド付き split（ログインシェル経由）");
            press(any, cx, "cmd-w");
            wait(cx, 500).await;

            // 64. tako panel CLI（開発不変条件）: 表示・ビュー・幅の roundtrip
            press(any, cx, "ctrl-u");
            type_text(
                any,
                cx,
                &format!(
                    "{cli} panel --show --view git --width 300 \
                     | grep -q '\"view\":\"git\"' && echo TAKO-PN-$((60+4))"
                ),
                true,
            );
            wait(cx, 1200).await;
            check(
                focused_contains(window, cx, "TAKO-PN-64"),
                "tako panel CLI の roundtrip",
            );
            let panel_synced = window
                .update(cx, |app, _, _| {
                    let ok = app.panel_visible
                        && app.panel_view == PanelView::Git
                        && (app.panel_width - 300.0).abs() < 1.0;
                    app.panel_visible = false;
                    ok
                })
                .unwrap_or(false);
            check(panel_synced, "panel 操作が UI 状態へ反映");

            // 64b. ファイルツリーの CLI / MCP 経路（FR-2.16.5。従来は cmd+B のみだった）:
            //      `tako panel --filetree on/off` で開閉でき、状態が応答 JSON に載る
            press(any, cx, "ctrl-u");
            type_text(
                any,
                cx,
                &format!(
                    "{cli} panel --filetree on | grep -q '\"filetree\":true' \
                     && echo TAKO-FT-$((60+4))b"
                ),
                true,
            );
            wait(cx, 1200).await;
            check(
                focused_contains(window, cx, "TAKO-FT-64b"),
                "tako panel --filetree の roundtrip",
            );
            let filetree_synced = window
                .update(cx, |app, _, _| {
                    // dispatch 経由でも root の cwd 同期込みで開く（行が出る状態）
                    let opened = app.filetree.visible && !app.filetree.roots().is_empty();
                    app.set_filetree(false);
                    opened && !app.filetree.visible
                })
                .unwrap_or(false);
            check(filetree_synced, "filetree 操作が UI 状態へ反映");

            // 65. 接続情報の上書き競合（2026-06-12 バグ (8) の回帰）: 一時インスタンスが
            //     current（control.json）を上書きして exit した状況を再現し、env なしの
            //     CLI が候補列経由で**生きているインスタンス**へ自動フォールバックする
            //     （置き場は TAKO_DISCOVERY_DIR の隔離ディレクトリ。メインには触らない）
            let poisoned = tako_control::discovery::write(&tako_control::discovery::ControlInfo {
                version: 1,
                pid: 999_999,
                socket: "/tmp/tako-dead-instance.sock".into(),
                token: "stale-token".into(),
                mcp_url: None,
            })
            .is_ok();
            check(poisoned, "current の汚染書き込み（バグ (8) 再現準備）");
            press(any, cx, "ctrl-u");
            type_text(
                any,
                cx,
                &format!(
                    "env -u TAKO_SOCKET -u TAKO_TOKEN {cli} list >/dev/null \
                     && echo TAKO-DSC-$((60+5))"
                ),
                true,
            );
            wait(cx, 1500).await;
            check(
                focused_contains(window, cx, "TAKO-DSC-65"),
                "汚染された current から生存インスタンスへフォールバック",
            );

            // 66. プレビューペイン（FR-3.2 / FR-3.3）: dispatch OpenFile（tako open / MCP
            //     tako_open_file / ファイルツリークリックと同一経路）でコードと md を開く。
            //     再利用・モード切替（dispatch + 目アイコントグル）・list 公開・close 片付け。
            //     OpenFile はセッション起動を伴わないため直接 dispatch でよい（pending_attach
            //     の後処理は不要 = 項目 56 の罠の対象外）
            let preview_dir =
                std::env::temp_dir().join(format!("tako-selftest-preview-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&preview_dir);
            std::fs::create_dir_all(&preview_dir).expect("一時ディレクトリを作れる");
            std::fs::write(preview_dir.join("hello.rs"), "fn main() {}\n").unwrap();
            std::fs::write(preview_dir.join("note.md"), "# Title\n\n- item\n").unwrap();
            let preview_ok = window
                .update(cx, |app, _, cx| {
                    let base = app.focused_pane().as_u64();
                    let open = |app: &mut TakoApp, path: String, mode| {
                        tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::OpenFile {
                                pane: Some(base),
                                path,
                                mode,
                            },
                            PaneOrigin::Cli,
                        )
                    };
                    // コードを開く: ペインが生え、PTY は起動しない。フォーカスは移る
                    let opened = open(
                        app,
                        preview_dir.join("hello.rs").display().to_string(),
                        None,
                    )
                    .expect("open_file は成功する");
                    let pane = opened["pane"].as_u64().expect("pane が返る");
                    let code_ok = opened["mode"].as_str() == Some("code")
                        && opened["created"].as_bool() == Some(true)
                        && pane != base
                        && !app.terminals.keys().any(|p| p.as_u64() == pane)
                        && app.focused_pane().as_u64() == pane
                        && matches!(
                            app.previews.values().next().map(|p| &p.content),
                            Some(preview::PreviewContent::Code(lines)) if !lines.is_empty()
                        );
                    // md を開く: 同じプレビューペインを再利用し、既定で markdown 表示
                    let opened = open(app, preview_dir.join("note.md").display().to_string(), None)
                        .expect("md の open_file は成功する");
                    let md_ok = opened["pane"].as_u64() == Some(pane)
                        && opened["created"].as_bool() == Some(false)
                        && opened["mode"].as_str() == Some("markdown")
                        && matches!(
                            app.previews.values().next().map(|p| &p.content),
                            Some(preview::PreviewContent::Markdown(blocks))
                                if matches!(blocks.first(),
                                    Some(preview::MdBlock::Heading { level: 1, .. }))
                        );
                    // dispatch の mode 指定（CLI --mode / MCP mode と同じ）でコード表示へ
                    let opened = open(
                        app,
                        preview_dir.join("note.md").display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Code),
                    )
                    .expect("mode 指定の open_file は成功する");
                    let mode_ok = opened["mode"].as_str() == Some("code");
                    // 目アイコンのトグル（UI 経路）で md レンダリングへ戻る
                    let pane_id = app
                        .previews
                        .keys()
                        .next()
                        .copied()
                        .expect("プレビューは 1 枚ある");
                    app.toggle_preview_mode(pane_id, cx);
                    let toggle_ok = app
                        .previews
                        .get(&pane_id)
                        .is_some_and(|p| p.mode == preview::PreviewMode::Markdown);
                    // list へ preview（path / mode）が公開される
                    let list = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::List,
                        PaneOrigin::Cli,
                    )
                    .expect("list は成功する");
                    let listed = list["tabs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|t| t["panes"].as_array().cloned().unwrap_or_default())
                        .find(|p| p["id"].as_u64() == Some(pane));
                    let list_ok = listed.as_ref().is_some_and(|p| {
                        p["preview"]["mode"].as_str() == Some("markdown")
                            && p["preview"]["path"]
                                .as_str()
                                .is_some_and(|s| s.ends_with("note.md"))
                    });
                    // close で片付く（previews からも消える）
                    let closed = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Close { pane: Some(pane) },
                        PaneOrigin::Cli,
                    )
                    .is_ok()
                        && app.previews.is_empty();
                    code_ok && md_ok && mode_ok && toggle_ok && list_ok && closed
                })
                .unwrap_or(false);
            check(preview_ok, "プレビューペインの open / 再利用 / モード切替 / close");

            // 66b. `tako open` CLI e2e（開発不変条件）: ペイン内シェルから実 CLI で開く。
            //      開いた後のフォーカスはプレビューペインに在るため、検証は app 状態で行う
            press(any, cx, "ctrl-u");
            type_text(
                any,
                cx,
                &format!(
                    "{cli} open {} > /dev/null",
                    preview_dir.join("note.md").display()
                ),
                true,
            );
            let mut cli_open_ok = false;
            for _ in 0..10 {
                wait(cx, 300).await;
                cli_open_ok = window
                    .update(cx, |app, _, _| {
                        app.previews
                            .values()
                            .any(|p| p.path.ends_with("note.md") && p.markdown_capable())
                    })
                    .unwrap_or(false);
                if cli_open_ok {
                    break;
                }
            }
            check(cli_open_ok, "tako open CLI でプレビューが開く");
            // 後片付け: プレビューを閉じる（フォーカスはターミナルへ戻る）
            let cleaned = window
                .update(cx, |app, _, _| {
                    let pane = app.previews.keys().next().copied();
                    if let Some(pane) = pane {
                        let _ = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Close {
                                pane: Some(pane.as_u64()),
                            },
                            PaneOrigin::Cli,
                        );
                    }
                    app.previews.is_empty()
                        && app.terminals.contains_key(&app.focused_pane())
                })
                .unwrap_or(false);
            check(cleaned, "プレビュー close 後はターミナルへフォーカスが戻る");
            let _ = std::fs::remove_dir_all(&preview_dir);

            // 67. ファイルツリーの「タブ = ワークスペース」（FR-3.1。2026-06-13 変更）:
            //     タブ内全ペインの cwd がワークスペースフォルダとして並ぶ（マルチルート）。
            //     cwd 違いのペインを生やし、ルート見出しが 2 つ以上になることを確認する
            let temp_pane = window
                .update(cx, |app, _, cx| {
                    let base = app.focused_pane().as_u64();
                    let split = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Split {
                            pane: Some(base),
                            direction: None,
                            ratio: None,
                            command: None,
                            cwd: Some("/private/tmp".into()),
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("split は成功する");
                    // 直接 dispatch のためセッション起動依頼はここで処理（項目 56 と同じ）
                    for (pane, options) in std::mem::take(&mut app.pending_attach) {
                        app.spawn_session(pane, options, cx)
                            .expect("一時ペインの PTY 起動は成功する");
                    }
                    app.set_filetree(true);
                    split["pane"].as_u64().expect("pane が返る")
                })
                .unwrap_or(0);
            let mut multiroot_ok = false;
            for _ in 0..20 {
                wait(cx, 300).await;
                multiroot_ok = window
                    .update(cx, |app, _, _| {
                        app.sync_filetree_roots();
                        let roots = app.filetree.roots();
                        let header_rows =
                            app.filetree.rows().iter().filter(|r| r.root).count();
                        roots.len() >= 2
                            && roots.iter().any(|r| r.ends_with("tmp"))
                            && header_rows == roots.len()
                    })
                    .unwrap_or(false);
                if multiroot_ok {
                    break;
                }
            }
            check(
                multiroot_ok,
                "タブ内全ペインの cwd がワークスペースフォルダとして並ぶ",
            );
            // 後片付け: ペインを閉じるとそのフォルダがツリーから消える
            let workspace_shrinks = window
                .update(cx, |app, _, _| {
                    let _ = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Close {
                            pane: Some(temp_pane),
                        },
                        PaneOrigin::Cli,
                    );
                    app.sync_filetree_roots();
                    let shrunk = !app.filetree.roots().iter().any(|r| r.ends_with("tmp"));
                    app.set_filetree(false);
                    shrunk && !app.filetree.visible
                })
                .unwrap_or(false);
            check(workspace_shrinks, "ペイン close でワークスペースフォルダが畳まれる");

            // 後片付け: 隔離した接続情報ディレクトリを消す
            if let Some(dir) = std::env::var_os("TAKO_DISCOVERY_DIR") {
                let _ = std::fs::remove_dir_all(dir);
            }

            println!("TAKO_APP_SELF_TEST_OK");
            std::process::exit(0);
        })
        .detach();
    }
}

/// 特殊キー → PTY 送出バイト列の総点検（バイトレベル検証）。
/// 実 IME / GUI を起動できない CI でもキーエンコードの退行を捕まえる
/// ホイールデルタの行換算。整数化できた行数と持ち越す端数を返す。
/// 方向が反転したら端数を捨てる（逆向きの貯金で初動が重くなるのを防ぐ）
fn accumulate_scroll(carry: f32, delta_lines: f32) -> (i32, f32) {
    let carry = if carry * delta_lines < 0.0 {
        0.0
    } else {
        carry
    };
    let total = carry + delta_lines;
    let lines = total.trunc() as i32;
    (lines, total - lines as f32)
}

#[cfg(test)]
mod scroll_tests {
    use super::accumulate_scroll;

    #[test]
    fn 微小デルタは蓄積されて行になる() {
        // 0.4 行ずつのゆっくりトラックパッド: 3 イベント目で 1 行出る
        let (l1, c1) = accumulate_scroll(0.0, 0.4);
        assert_eq!(l1, 0);
        let (l2, c2) = accumulate_scroll(c1, 0.4);
        assert_eq!(l2, 0);
        let (l3, c3) = accumulate_scroll(c2, 0.4);
        assert_eq!(l3, 1);
        assert!((c3 - 0.2).abs() < 1e-5);
    }

    #[test]
    fn 大きなデルタは即時に複数行になる() {
        let (lines, carry) = accumulate_scroll(0.0, 2.7);
        assert_eq!(lines, 2);
        assert!((carry - 0.7).abs() < 1e-5);
        let (lines, carry) = accumulate_scroll(0.0, -3.2);
        assert_eq!(lines, -3);
        assert!((carry + 0.2).abs() < 1e-5);
    }

    #[test]
    fn スクロールバーは表示維持後にフェードする() {
        use super::{scrollbar_alpha, SCROLLBAR_FADE_MS, SCROLLBAR_SHOW_MS};
        assert_eq!(scrollbar_alpha(0), 1.0);
        assert_eq!(scrollbar_alpha(SCROLLBAR_SHOW_MS), 1.0);
        let mid = scrollbar_alpha(SCROLLBAR_SHOW_MS + SCROLLBAR_FADE_MS / 2);
        assert!(mid > 0.0 && mid < 1.0, "フェード中間で半透明: {mid}");
        assert_eq!(scrollbar_alpha(SCROLLBAR_SHOW_MS + SCROLLBAR_FADE_MS), 0.0);
        assert_eq!(scrollbar_alpha(u128::MAX), 0.0);
    }

    #[test]
    fn 方向反転で端数は捨てる() {
        // 上方向の貯金 0.9 があっても、下へ動かした瞬間はゼロから数え直す
        let (lines, carry) = accumulate_scroll(0.9, -0.4);
        assert_eq!(lines, 0);
        assert!((carry + 0.4).abs() < 1e-5);
    }
}

#[cfg(test)]
mod keystroke_tests {
    use super::*;

    fn ks(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: key.into(),
            key_char: None,
        }
    }
    fn ks_char(key: &str, ch: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: key.into(),
            key_char: Some(ch.into()),
        }
    }
    fn ks_ctrl(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            key: key.into(),
            key_char: None,
        }
    }

    #[test]
    fn 特殊キーは正しいバイト列を送る() {
        // Backspace は DEL(0x7f)。BS(0x08) ではない（macOS の stty erase 既定が ^?）
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("backspace")),
            Some(b"\x7f".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("enter")),
            Some(b"\r".to_vec())
        );
        assert_eq!(keystroke_to_bytes_legacy(&ks("tab")), Some(b"\t".to_vec()));
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("escape")),
            Some(b"\x1b".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("up")),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("down")),
            Some(b"\x1b[B".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("right")),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("left")),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("home")),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("end")),
            Some(b"\x1b[F".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("pageup")),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("pagedown")),
            Some(b"\x1b[6~".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks("delete")),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn imeのrange先頭は擬似ドキュメント内へ解釈する() {
        // 正常系（marked text 内のオフセット）はそのまま
        assert_eq!(clamp_ime_range_start(0, 4, None), 0);
        assert_eq!(clamp_ime_range_start(4, 4, Some(&(2..4))), 4);
        // 文書全体基準のオフセット（ライブ変換）は注目文節の先頭へ
        assert_eq!(clamp_ime_range_start(100, 4, Some(&(2..4))), 2);
        // 注目文節が無ければ末尾（挿入点）へ
        assert_eq!(clamp_ime_range_start(100, 4, None), 4);
    }

    #[test]
    fn 修飾付き機能キーはxterm形式で送る() {
        // shift+up = CSI 1;2A（従来は修飾が無視されていた）
        assert_eq!(
            keystroke_to_bytes_legacy(&ks_shift("up")),
            Some(b"\x1b[1;2A".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks_shift("delete")),
            Some(b"\x1b[3;2~".to_vec())
        );
        // disambiguate なしの shift+enter はレガシーどおり \r（区別不能）
        assert_eq!(
            keystroke_to_bytes_legacy(&ks_shift("enter")),
            Some(b"\r".to_vec())
        );
    }

    #[test]
    fn disambiguate有効時は修飾付きenterをcsi_uで送る() {
        // Claude Code TUI の Shift+Enter 改行（kitty keyboard protocol disambiguate）
        assert_eq!(
            keystroke_to_bytes(&ks_shift("enter"), CsiUMode::Full),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_ctrl("enter"), CsiUMode::Full),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("tab"), CsiUMode::Full),
            Some(b"\x1b[9;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("backspace"), CsiUMode::Full),
            Some(b"\x1b[127;2u".to_vec())
        );
        // Esc は単押しでも CSI u（アプリが kitty 要求済み = disambiguate の仕様）
        assert_eq!(
            keystroke_to_bytes(&ks("escape"), CsiUMode::Full),
            Some(b"\x1b[27u".to_vec())
        );
        // 無修飾 Enter / Tab / Backspace はレガシーのまま
        assert_eq!(
            keystroke_to_bytes(&ks("enter"), CsiUMode::Full),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks("tab"), CsiUMode::Full),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks("backspace"), CsiUMode::Full),
            Some(b"\x7f".to_vec())
        );
    }

    #[test]
    fn バックエンドペインはesc単押しを素のescで送る() {
        // tmux バックエンド強制（ModifiedOnly）: 修飾付きキーは CSI u を維持しつつ、
        // Esc 単押しは素の \e。tmux は CSI 27u を内側ペインの kitty 要求に関係なく
        // 素通しするため、CSI u 非対応アプリに「27u」が文字として挿入される
        // （2026-06-12 実機バグの再発防止）
        assert_eq!(
            keystroke_to_bytes(&ks("escape"), CsiUMode::ModifiedOnly),
            Some(b"\x1b".to_vec())
        );
        // Shift+Enter（生命線）は CSI u のまま
        assert_eq!(
            keystroke_to_bytes(&ks_shift("enter"), CsiUMode::ModifiedOnly),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("tab"), CsiUMode::ModifiedOnly),
            Some(b"\x1b[9;2u".to_vec())
        );
        // 修飾付き Esc はレガシー形式だと区別不能なので CSI u
        assert_eq!(
            keystroke_to_bytes(&ks_shift("escape"), CsiUMode::ModifiedOnly),
            Some(b"\x1b[27;2u".to_vec())
        );
    }

    #[test]
    fn ctrl英字はc0制御コードを送る() {
        assert_eq!(keystroke_to_bytes_legacy(&ks_ctrl("a")), Some(vec![0x01]));
        assert_eq!(keystroke_to_bytes_legacy(&ks_ctrl("c")), Some(vec![0x03]));
        assert_eq!(keystroke_to_bytes_legacy(&ks_ctrl("u")), Some(vec![0x15]));
        assert_eq!(keystroke_to_bytes_legacy(&ks_ctrl("z")), Some(vec![0x1a]));
    }

    fn ks_shift(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            key: key.into(),
            key_char: None,
        }
    }

    /// disambiguate なし（レガシー端末モード）の変換
    fn keystroke_to_bytes_legacy(ks: &Keystroke) -> Option<Vec<u8>> {
        keystroke_to_bytes(ks, CsiUMode::Off)
    }

    #[test]
    fn 印字可能文字はkey_charをそのまま送る() {
        assert_eq!(
            keystroke_to_bytes_legacy(&ks_char("a", "a")),
            Some(b"a".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks_char("space", " ")),
            Some(b" ".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes_legacy(&ks_char("a", "あ")),
            Some("あ".as_bytes().to_vec())
        );
        // key_char の無い未知キーは送出しない
        assert_eq!(keystroke_to_bytes_legacy(&ks("f5")), None);
    }
}

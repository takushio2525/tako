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
mod file_icons;
mod filetree;
mod preview;
mod video_player;

use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::{
    actions, canvas, div, fill, point, prelude::*, px, quad, relative, size, svg, App, BorderStyle,
    Bounds, BoxShadow, ClipboardItem, Context, CursorStyle, DragMoveEvent, ElementInputHandler,
    EntityInputHandler, FocusHandle, Font, FontStyle, FontWeight, HighlightStyle, Hsla, KeyBinding,
    Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Rgba, ScrollDelta, ScrollWheelEvent, SharedString, Size, StrikethroughStyle, StyledText,
    TextRun, TextStyle, UTF16Selection, UnderlineStyle, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;
use tako_control::{ControlHost, IncomingRequest, IpcServer, McpServer};
use tako_core::{
    ratio_for_position, AgentMetrics, CommandState, Pane, PaneId, PaneOrigin, Rect, SelectionKind,
    SessionNotice, SpawnOptions, SplitAxis, SplitDirection, TabId, TerminalSession, Theme,
    TitleSource, Workspace, WorkspaceError,
};

/// 新規セッションの初期グリッド。最初の render で実寸へリサイズされる
const INITIAL_COLS: usize = 80;
const INITIAL_ROWS: usize = 24;

/// タブバーの高さ（px）
const TAB_BAR_HEIGHT: f32 = 40.0;
/// ペイン枠線の太さ（px）
const PANE_BORDER: f32 = 1.0;
/// ペイン内側の余白（px。デザインスペック: 12–14px content padding）
const PANE_PADDING: f32 = 10.0;
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
const SIDEBAR_WIDTH: f32 = 244.0;

/// 右サイドバー（情報パネル）の既定幅・最小幅（px。ドラッグで可変）
const PANEL_DEFAULT_WIDTH: f32 = 320.0;
const PANEL_MIN_WIDTH: f32 = 220.0;

/// ペイン上部タイトルバーの高さ（px。デザインスペック: 30px）
const PANE_TITLE_BAR: f32 = 30.0;

/// 下部ステータスバーの高さ（px。FR-2.16.4。Zed / VSCode 風）
const STATUS_BAR_HEIGHT: f32 = 30.0;

/// バックグラウンドドロワーの既定高さ（px。FR-2.15）。横並びプレビュー（実画面サムネイル）が
/// 読めるだけの高さを確保する
const DRAWER_DEFAULT_HEIGHT: f32 = 240.0;

/// バックグラウンドドロワー上端のヘッダ行の高さ（px）
const DRAWER_HEADER_HEIGHT: f32 = 22.0;

/// バックグラウンドドロワーのタブ別グループ見出し行の高さ（px。FR-2.15.6）
const DRAWER_GROUP_HEADER: f32 = 16.0;

/// バックグラウンドプレビューカード 1 枚の幅（px。横並び + 横スクロール）
const BG_CARD_WIDTH: f32 = 300.0;

/// タブツリーのホバープレビュー / ピン留めウィンドウの既定サイズ（px。FR-2.16.13）。
/// 実画面サムネイル（terminal_screen_lines）をクリップ表示する箱の寸法
const PREVIEW_POPUP_W: f32 = 380.0;
const PREVIEW_POPUP_H: f32 = 240.0;
/// ピン留めウィンドウのタイトルバー高さ（px。ドラッグ移動 + × の操作帯）
const PIN_TITLE_BAR: f32 = 20.0;
/// ピン留めウィンドウの既定サイズ（px。ホバーポップアップより小さい常駐窓）
const PIN_W: f32 = 280.0;
const PIN_H: f32 = 180.0;

/// 右サイドバー情報パネルの内部タブ（固定タブ 0 個方針。2026-06-12）。
/// FR-2.16.6 で agents は tmux ビューへ統合済み。Git は git graph（FR-3.6）の
/// 実装までプレースホルダ（パネルは切り替え式コンテナとして設計）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PanelView {
    #[default]
    Tmux,
    Git,
}

/// claude TUI へのプロンプト送信フローの状態
#[derive(Debug)]
enum PromptFlowState {
    /// alt_screen 遷移待ち
    WaitAltScreen,
    /// claude TUI の ❯ プロンプト表示待ち
    WaitPromptReady,
    /// プロンプトテキスト送信済み、入力欄への反映待ち
    WaitTextEchoed,
    /// Enter 送信済み、処理開始確認待ち
    WaitProcessing,
    /// 完了
    Done,
}

/// claude TUI へのプロンプト送信ステートマシン
#[derive(Debug)]
struct PromptFlow {
    pane: PaneId,
    prompt: String,
    state: PromptFlowState,
    created_at: std::time::Instant,
    /// 現在のステートに遷移した時刻（ステート内タイムアウト用）
    state_entered_at: std::time::Instant,
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
        ActivateTab9,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        SelectAll
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
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ResetZoom, None),
        KeyBinding::new("cmd-a", SelectAll, None),
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
    /// 実測したセル寸法（最初の render で確定。デフォルトフォントサイズ用）
    cell_size: Option<Size<Pixels>>,
    /// ペインごとのフォントサイズオーバーライド（未設定ならテーマ既定）
    pane_font_sizes: HashMap<PaneId, f32>,
    /// ペインごとのセル寸法キャッシュ（フォントサイズ変更時に無効化）
    pane_cell_sizes: HashMap<PaneId, Size<Pixels>>,
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
    /// セッション起動後に遅延書き込みするデータ（attach_session が非同期のため）
    pending_writes: Vec<(PaneId, Vec<u8>)>,
    /// alt_screen 遷移後に書き込むデータ（非プロンプト用の汎用遅延書き込み）
    alt_screen_writes: Vec<(PaneId, Vec<u8>, std::time::Instant)>,
    /// claude TUI へのプロンプト送信ステートマシン
    prompt_flows: Vec<PromptFlow>,
    /// dispatch 中に依頼されたプレビューの background ハイライト（ペイン, パス, 生テキスト）
    pending_highlights: Vec<(PaneId, std::path::PathBuf, String)>,
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
    /// バックエンドセッション内の window 一覧（tmux ポーリングで更新。2+ window のみ保持）
    backend_windows: HashMap<PaneId, Vec<tako_core::TmuxWindow>>,
    /// tmux window のキャプチャテキスト（ホバープレビュー用。ポーリングで非アクティブ window を取得）
    window_captures: HashMap<(PaneId, u32), Vec<String>>,
    /// 直近に保存したレイアウトの JSON（変化したときだけ書き込むための比較用）
    last_saved_layout: Option<String>,
    /// OS ウィンドウの現フレーム（render で採取し layout 保存に含める。FR-5）
    window_frame: Option<tako_control::layout::WindowFrame>,
    /// tmux ビューのアコーディオン折りたたみ状態（FR-2.16.14。折りたたむと配下の
    /// バックグラウンド行 + バックグラウンドを隠す）。タブ並べ替え / クローズに強い TabId キー
    collapsed_tmux_tabs: std::collections::HashSet<TabId>,
    /// TmuxOpen で作成されたペインの監視対象。対象セッションが消滅したら
    /// ペインを自動クローズする（ポーリングで検知）
    tmux_view_panes: HashMap<PaneId, TmuxViewTarget>,
    /// ファイルツリーのコンテキストメニュー（FR-3.12）
    context_menu: Option<ContextMenu>,
    /// ファイルツリーのインライン編集
    inline_edit: Option<InlineEdit>,
    /// D&D 中のペイロード種別（FR-2.16.10 / FR-3.11）。on_drag 開始でセット、
    /// drop / mouse-up でクリア。gpui の active_drag は型を公開しないため自前で追跡し、
    /// ドロップ先オーバーレイの生成判定 + ラベル出し分けに使う
    drag_kind: Option<DragKind>,
    /// ドラッグ中のドロップ先（ペイン, 挿入位置）。挿入プレビュー表示の状態
    drop_target: Option<(PaneId, DropZone)>,
    /// git パネルのデータ（FR-3.6。cwd 連動で 2 秒ポーリング更新）
    git_data: Option<GitPanelData>,
    /// git パネルで選択中のコミット（diff 表示用）
    git_selected_commit: Option<String>,
    /// git パネルのアコーディオン折りたたみ
    git_collapsed: GitCollapsed,
    /// バックグラウンドドロワーの表示状態（FR-2.15。下部ステータスバーのボタンでトグル）
    drawer_visible: bool,
    /// バックグラウンドドロワーの高さ（px）
    drawer_height: f32,
    /// バックグラウンド内のペインの kill 確認待ち
    bg_pending_kill: Option<PaneId>,
    /// サイドバー tmux ビューでホバー中のプレビュー（FR-2.16.13。バックグラウンド行 /
    /// 閉じたタブグループの中身をマウス位置のポップアップで覗く）
    hover_preview: Option<HoverPreview>,
    /// ピン留めされた常駐プレビュー（FR-2.16.15。アプリ内フローティングウィンドウ）
    pinned_previews: Vec<PinnedPreview>,
    /// ドラッグ移動中のピン（対象 + 掴んだ位置からピン左上までのオフセット px）
    dragging_pin: Option<(PreviewTarget, Point<Pixels>)>,
    /// AVFoundation 動画プレイヤー（ペインごと。Video プレビューで「再生」した時に生成）
    video_players: HashMap<PaneId, video_player::VideoPlayer>,
    /// 動画フレーム更新ティッカーの稼働中フラグ（再生中ペインがある間だけ回す）
    video_ticker: bool,
    /// 全ペインから集約した Claude エージェントメトリクス（ctx/usage。ポーリングで更新）
    agent_metrics: AgentMetrics,
    /// 動画フレームの描画キャッシュ（frame_gen で世代管理: 新フレーム準備完了まで前フレームを表示）
    video_frame_cache: HashMap<PaneId, (u64, std::sync::Arc<gpui::RenderImage>)>,
    /// シークバー要素の実測 bounds（paint 時に canvas で記録）
    video_seek_bar_bounds: HashMap<PaneId, Bounds<Pixels>>,
    /// プレビューペインのテキスト選択
    preview_selections: HashMap<PaneId, PreviewSelection>,
    /// プレビューで選択操作中のペイン
    preview_selecting: Option<PaneId>,
    /// プレビューの行ごとの bounds（paint 時に canvas で記録。選択のヒット判定用）
    preview_line_bounds: HashMap<PaneId, Vec<Bounds<Pixels>>>,
    /// プレビューの行ごとのプレーンテキスト（選択テキスト抽出用）
    preview_line_texts: HashMap<PaneId, Vec<String>>,
}

/// git パネルのデータスナップショット（FR-3.6 / FR-3.9）
#[derive(Debug, Clone)]
struct GitPanelData {
    repo_root: String,
    branch: String,
    upstream: String,
    commits: Vec<tako_core::GitCommit>,
    branches: Vec<tako_core::GitBranch>,
    status: Vec<tako_core::GitStatusEntry>,
    diff_files: Vec<tako_core::DiffFile>,
    graph: tako_core::GraphLayout,
}

/// git パネルのアコーディオン折りたたみ状態
#[derive(Debug, Clone, Default)]
struct GitCollapsed {
    branches: bool,
    changes: bool,
    commits: bool,
    diff: bool,
}

/// TmuxOpen でペインに表示している外部 tmux セッションの監視情報
#[derive(Debug, Clone)]
struct TmuxViewTarget {
    /// 監視・再 attach 対象の**元セッション**（ラッパー名は入れない）。
    /// これが消滅したらペインを自動クローズする
    session: String,
    /// 表示用の `tako-view-*` grouped session 名。ペイン close 時にこれを kill する。
    /// `None` = 元セッションを直接 attach した（復帰経路）ので close 時も kill しない
    wrapper: Option<String>,
    /// 元セッションが居る tmux サーバーの socket（`-L` 値。既定サーバーは None）
    socket: Option<String>,
}

/// プレビューペインのテキスト選択状態
#[derive(Debug, Clone)]
struct PreviewSelection {
    anchor: (usize, usize),
    head: (usize, usize),
}

impl PreviewSelection {
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor.0 < self.head.0
            || (self.anchor.0 == self.head.0 && self.anchor.1 <= self.head.1)
        {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    fn range_for_line(&self, line: usize, line_len: usize) -> Option<(usize, usize)> {
        let (start, end) = self.ordered();
        if line < start.0 || line > end.0 {
            return None;
        }
        let col_start = if line == start.0 { start.1 } else { 0 };
        let col_end = if line == end.0 {
            end.1.min(line_len)
        } else {
            line_len
        };
        if col_start >= col_end && !(line > start.0 && line < end.0) {
            return None;
        }
        Some((col_start, col_end))
    }
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
    /// ペインを保持する tmux バックエンドセッション名（Phase 5.5。非永続化ペインは None）
    backend: Option<String>,
    /// 補足タイトル（OSC タイトル → tako ペインタイトルの順でフォールバック）
    detail_title: String,
}

/// 統合 tmux ビューのタブ 1 枠分（FR-2.16.6。タブ名ラベル付き四角枠 + 全ペイン入れ子）
#[derive(Debug, Clone)]
struct TmuxViewTabGroup {
    tab: TabId,
    title: String,
    rows: Vec<AgentEntry>,
    /// このタブのペイン内で attach 中の外部 tmux セッション（FR-2.16.9）
    sessions: Vec<AttachedTmuxSession>,
    /// このタブ由来のバックグラウンドペイン（FR-2.15.6。タブ別分離してバックグラウンド表示する）
    backgrounded: Vec<BackgroundEntry>,
}

/// タブ別バックグラウンド表示の 1 ペイン分（FR-2.15.6）。バックグラウンドペインは常にバックグラウンド扱い
#[derive(Debug, Clone)]
struct BackgroundEntry {
    pane: PaneId,
    label: String,
    state: CommandState,
}

/// 閉じた由来タブのバックグラウンドペイン群（FR-2.15.6）。由来タブが既に存在しないバックグラウンドペインを
/// 「タブ <名前>（閉じたタブ）」としてまとめて表示する
#[derive(Debug, Clone)]
struct ClosedOriginBackgroundGroup {
    /// 由来タブ ID（ホバープレビュー / ピンの対象解決に使う。FR-2.16.16）
    tab: TabId,
    title: String,
    entries: Vec<BackgroundEntry>,
}

/// ホバープレビュー / ピン留めの対象（FR-2.16.13）。サイドバー tmux ビューの
/// バックグラウンド行（単一ペイン）や閉じたタブグループ全体の中身を覗くために使う。
/// MCP / CLI からも同じ語彙で操作できるよう core の ID を直接持つ（設計原則 5）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewTarget {
    /// 単一ペイン（バックグラウンド行のホバー。FR-2.16.13）
    Pane(PaneId),
    /// 閉じたタブグループ全体（由来タブ ID。FR-2.16.16。グループ内の全バックグラウンドペインを
    /// 並べてプレビューする）
    ClosedGroup(TabId),
    /// バックエンドセッション内の特定 tmux window（ペイン ID + window index）。
    /// ペインが表示中の window 以外をプレビュー / ピン留めする
    TmuxWindow(PaneId, u32),
}

/// ホバー中のプレビュー（FR-2.16.13）。マウス位置を起点にポップアップを出す。
/// ポップアップ自体は読み取り専用にし、ピン留め操作は行 / カード側のボタンへ置く
/// （ポップアップへマウスを移すと行ホバーが切れる問題を避けるため）
#[derive(Debug, Clone, Copy)]
struct HoverPreview {
    target: PreviewTarget,
    /// ホバー開始時のマウス位置（ウィンドウ座標 px）。ここを起点に左側へポップアップを出す
    anchor: Point<Pixels>,
}

/// ピン留めされた常駐プレビュー（FR-2.16.15）。アプリ内フローティングウィンドウとして
/// 残り続け、ライブ更新する。`pos` はウィンドウ座標の左上（タイトルバー D&D で動かす）
#[derive(Debug, Clone, Copy)]
struct PinnedPreview {
    target: PreviewTarget,
    pos: Point<Pixels>,
}

/// ピン / プレビュー要素の安定 ID（GPUI の要素 id 用）。ペイン ID とタブ ID の
/// 衝突を避けるためグループには最上位ビットを立てる
fn pin_key(target: PreviewTarget) -> u64 {
    match target {
        PreviewTarget::Pane(id) => id.as_u64(),
        PreviewTarget::ClosedGroup(tab) => tab.as_u64() | (1 << 63),
        PreviewTarget::TmuxWindow(pane, win) => pane.as_u64() ^ ((win as u64) << 32) | (1 << 62),
    }
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

/// D&D ペイロード: 統合 tmux ビューのセッション/window 行（FR-2.16.10。`on_drop` の型キー）
#[derive(Debug, Clone)]
struct TmuxSessionDrag {
    name: String,
    socket: Option<String>,
    /// Some(index) なら特定 window を attach、None ならセッション全体
    window: Option<u32>,
}

/// D&D ペイロード: ファイルツリーのファイル行（FR-3.11。`on_drop` の型キー）
#[derive(Debug, Clone)]
struct FileDrag {
    path: std::path::PathBuf,
}

/// D&D ペイロード: ペインのタイトルバー（FR-1.10。iTerm2 流のペイン移動）
#[derive(Debug, Clone, Copy)]
struct PaneDrag {
    pane: PaneId,
}

/// ファイルツリーの右クリックコンテキストメニュー（FR-3.12）
struct ContextMenu {
    path: std::path::PathBuf,
    is_dir: bool,
    position: Point<Pixels>,
}

/// ファイルツリーのインライン編集（FR-3.12）
#[derive(Clone)]
struct InlineEdit {
    parent: std::path::PathBuf,
    kind: InlineEditKind,
    text: String,
    cursor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InlineEditKind {
    Rename,
    NewFile,
    NewDir,
}

/// D&D ペイロード: バックグラウンドのペイン（FR-2.15.3。ドロワーからペインエリアへ復帰）
#[derive(Debug, Clone, Copy)]
struct BackgroundPaneDrag {
    pane: PaneId,
}

/// D&D ペイロード: タブをバックグラウンドへバックグラウンド（FR-2.15 タブ D&D バックグラウンド）
#[derive(Debug, Clone, Copy)]
struct TabDrag {
    tab: TabId,
}

/// ドラッグ中のペイロード種別（`TakoApp::drag_kind`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragKind {
    TmuxSession,
    File,
    Pane,
    BackgroundPane,
    Tab,
}

/// ドロップ先の挿入位置。上下左右 = その方向へ分割、Center = ファイル D&D のみで
/// 「direction なし = FR-3.2 の既存プレビュー再利用セマンティクス」
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropZone {
    Left,
    Right,
    Up,
    Down,
    Center,
}

/// カーソル位置 → ドロップゾーンの判定（純関数。fx / fy はペイン矩形内の正規化位置）。
/// `center_allowed` 時は中央 40% 矩形を Center、外周は最も近い辺の象限にする
fn drop_zone(fx: f32, fy: f32, center_allowed: bool) -> DropZone {
    if center_allowed && (0.3..=0.7).contains(&fx) && (0.3..=0.7).contains(&fy) {
        return DropZone::Center;
    }
    [
        (fx, DropZone::Left),
        (1.0 - fx, DropZone::Right),
        (fy, DropZone::Up),
        (1.0 - fy, DropZone::Down),
    ]
    .into_iter()
    .min_by(|a, b| a.0.total_cmp(&b.0))
    .map(|(_, zone)| zone)
    .expect("候補は常に 4 つある")
}

/// ドロップゾーン → 分割方向（Center は呼び出し側で direction なしに落とすこと）
fn zone_to_direction(zone: DropZone) -> tako_control::protocol::Direction {
    use tako_control::protocol::Direction;
    match zone {
        DropZone::Left => Direction::Left,
        DropZone::Right | DropZone::Center => Direction::Right,
        DropZone::Up => Direction::Up,
        DropZone::Down => Direction::Down,
    }
}

/// ドラッグ中にカーソルへ追従するゴーストチップ（`on_drag` のコンストラクタが返す）
struct DragGhost {
    label: String,
    theme: Theme,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgba(self.theme.tab_bar_background))
            .border_1()
            .border_color(hsla(self.theme.accent))
            .text_size(px(11.0))
            .text_color(hsla(self.theme.foreground))
            .child(SharedString::from(self.label.clone()))
    }
}

impl TakoApp {
    fn new(cx: &mut Context<Self>) -> Self {
        // IPC（Layer 1）と MCP（Layer 2）の受け口。最初のセッション起動より前に立てて
        // ルートペインのシェルにも TAKO_SOCKET / TAKO_MCP_URL / TAKO_TOKEN を注入できるようにする。
        // 認証トークンは両者で共有する（FR-2.3.4）
        let (control_tx, mut control_rx) = unbounded::<IncomingRequest>();
        let token = match tako_control::load_or_create_token() {
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
        let mcp = token.as_ref().and_then(|token| {
            match McpServer::start(control_tx.clone(), token.clone()) {
                Ok(server) => Some(server),
                Err(e) => {
                    eprintln!(
                        "warning: MCP サーバーを起動できない（エージェント連携は使えない）: {e}"
                    );
                    None
                }
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
        let mut collapsed_tmux_tabs: std::collections::HashSet<TabId> =
            std::collections::HashSet::new();
        let workspace = if tmux_persist && tako_core::tmux_backend::available() {
            tako_control::layout::load()
                .and_then(|file| {
                    // 折りたたみ状態（FR-2.16.14）を控えてから復元する。タブ ID は
                    // Phase 5.5 で同一値に復元されるので再起動後も対応が保たれる
                    collapsed_tmux_tabs = file
                        .collapsed
                        .iter()
                        .map(|id| TabId::from_raw(*id))
                        .collect();
                    tako_control::layout::restore(&file)
                })
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
            pane_font_sizes: HashMap::new(),
            pane_cell_sizes: HashMap::new(),
            selecting: None,
            pane_text_areas: Vec::new(),
            ipc,
            mcp,
            token,
            pending_attach: Vec::new(),
            pending_writes: Vec::new(),
            alt_screen_writes: Vec::new(),
            prompt_flows: Vec::new(),
            pending_highlights: Vec::new(),
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
            collapsed_tmux_tabs,
            tmux_view_panes: HashMap::new(),
            context_menu: None,
            inline_edit: None,
            filetree: filetree::FileTree::default(),
            previews: HashMap::new(),
            autorename: autorename::AutoRenamer::new(initial_auto_rename()),
            port_detect: initial_port_detect(),
            port_suggestions: Vec::new(),
            dismissed_ports: std::collections::HashSet::new(),
            tmux_persist,
            backend_sessions: HashMap::new(),
            backend_windows: HashMap::new(),
            window_captures: HashMap::new(),
            last_saved_layout: None,
            window_frame: None,
            drag_kind: None,
            drop_target: None,
            git_data: None,
            git_selected_commit: None,
            git_collapsed: GitCollapsed::default(),
            drawer_visible: false,
            drawer_height: DRAWER_DEFAULT_HEIGHT,
            bg_pending_kill: None,
            hover_preview: None,
            pinned_previews: Vec::new(),
            dragging_pin: None,
            agent_metrics: AgentMetrics::default(),
            video_players: HashMap::new(),
            video_ticker: false,
            video_frame_cache: HashMap::new(),
            video_seek_bar_bounds: HashMap::new(),
            preview_selections: HashMap::new(),
            preview_selecting: None,
            preview_line_bounds: HashMap::new(),
            preview_line_texts: HashMap::new(),
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
                        "image" => preview::PreviewMode::Image,
                        "pdf" => preview::PreviewMode::Pdf,
                        "video" => preview::PreviewMode::Video,
                        _ => preview::PreviewMode::Code,
                    };
                    let path = std::path::Path::new(&p.path);
                    let (state, raw) = preview::load_fast(path, mode);
                    if let Some(text) = raw {
                        app.pending_highlights
                            .push((pane, path.to_path_buf(), text));
                    }
                    app.previews.insert(pane, state);
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
            // 復元時のプレビューも background でハイライトする
            for (pane, path, text) in std::mem::take(&mut app.pending_highlights) {
                app.spawn_highlight(pane, path, text, cx);
            }
        }

        // 起動時の orphan 一括クリーンアップ（FR-2.16.11）。復元で backend_sessions が
        // 出揃った後に実行し、前回クラッシュ等で取り残された detached・非 grouped の
        // backend セッションだけを掃除する（現存・バックグラウンド・表示中ビューは protected で除外）
        let cleaned = app.cleanup_orphan_tmux();
        if !cleaned.is_empty() {
            eprintln!(
                "info: orphan tmux セッションを {} 件クリーンアップした",
                cleaned.len()
            );
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
                    // セッション起動後の遅延書き込み（orchestrator spawn の claude 起動コマンド等）
                    for (pane, data) in std::mem::take(&mut app.pending_writes) {
                        if let Some(session) = app.terminals.get(&pane) {
                            session.write(data);
                        }
                    }
                    // alt_screen 遷移待ちの遅延書き込み（orchestrator spawn のプロンプト送信）
                    app.flush_alt_screen_writes();
                    // プレビューの syntect ハイライトを background で実行する
                    for (pane, path, text) in std::mem::take(&mut app.pending_highlights) {
                        app.spawn_highlight(pane, path, text, cx);
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

        // alt_screen 遷移待ち + プロンプトフロー駆動のポーリング（500ms 間隔）。
        // spawn 直後は他の IPC リクエストが来ない可能性があるため、短間隔で回す
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let ok = this.update(cx, |app: &mut TakoApp, _| {
                if !app.alt_screen_writes.is_empty() {
                    app.flush_alt_screen_writes();
                }
                if !app.prompt_flows.is_empty() {
                    app.drive_prompt_flows();
                }
            });
            if ok.is_err() {
                break;
            }
        })
        .detach();

        // 2 秒毎の定期更新: tmux 一覧（FR-2.13）+ ファイルツリー（FR-3.1）+ git（FR-3.6）。
        // 外部コマンド実行は background で行い、UI スレッドではコンテキスト収集と結果適用のみ
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            // ① main thread: tmux コンテキスト + filetree 対象 + view 監視対象 + git を収集（高速）
            let prep = this.update(cx, |app: &mut TakoApp, _| {
                let tmux_ctx = if app.panel_visible && app.panel_view == PanelView::Tmux {
                    Some(app.collect_tmux_context())
                } else {
                    None
                };
                app.sync_filetree_roots();
                app.refresh_agent_metrics();
                app.save_layout();
                let filetree_targets = if app.filetree.visible {
                    Some(app.filetree.refresh_targets())
                } else {
                    None
                };
                let view_targets: Vec<(PaneId, String, Option<String>)> = app
                    .tmux_view_panes
                    .iter()
                    .map(|(id, t)| (*id, t.session.clone(), t.socket.clone()))
                    .collect();
                let git_cwd = if app.panel_visible && app.panel_view == PanelView::Git {
                    app.active_tab_cwd()
                } else {
                    None
                };
                let git_selected = app.git_selected_commit.clone();
                (
                    tmux_ctx,
                    filetree_targets,
                    view_targets,
                    git_cwd,
                    git_selected,
                )
            });
            let Ok((tmux_ctx, filetree_targets, view_targets, git_cwd, git_selected)) = prep else {
                break;
            };
            // ② background: tmux コマンド実行 + ディレクトリ読み取り
            let tmux_result = if let Some(ctx) = tmux_ctx {
                let task = cx
                    .background_executor()
                    .spawn(async move { tako_control::fetch_tmux_sessions(&ctx) });
                Some(task.await)
            } else {
                None
            };
            if let Some(sessions) = tmux_result {
                let ok = this.update(cx, |app: &mut TakoApp, cx| {
                    app.tmux_sessions = sessions;
                    app.sync_backend_windows();
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
            }
            // TmuxOpen ペインの監視: 対象セッションが消滅したら自動クローズ
            if !view_targets.is_empty() {
                let dead_panes: Vec<PaneId> = {
                    let targets = view_targets;
                    cx.background_executor()
                        .spawn(async move {
                            targets
                                .into_iter()
                                .filter(|(_, session, socket)| {
                                    !tako_core::tmux::has_session(socket.as_deref(), session)
                                })
                                .map(|(id, _, _)| id)
                                .collect()
                        })
                        .await
                };
                if !dead_panes.is_empty() {
                    let ok = this.update(cx, |app: &mut TakoApp, cx| {
                        for pane_id in dead_panes {
                            if app.tmux_view_panes.contains_key(&pane_id) {
                                app.remove_pane(pane_id, cx);
                            }
                        }
                    });
                    if ok.is_err() {
                        break;
                    }
                }
            }
            if let Some(targets) = filetree_targets {
                let git_roots: Vec<std::path::PathBuf> =
                    targets.iter().filter(|p| p.is_dir()).cloned().collect();
                let task = cx
                    .background_executor()
                    .spawn(async move { filetree::scan_dirs(&targets) });
                let git_task = cx
                    .background_executor()
                    .spawn(async move { filetree::scan_git_status(&git_roots) });
                let results = task.await;
                let git_status = git_task.await;
                let ok = this.update(cx, |app: &mut TakoApp, cx| {
                    let mut changed = app.filetree.apply_refresh(results);
                    changed |= app.filetree.apply_git_status(git_status);
                    if changed {
                        cx.notify();
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
            // ③ background: git データ取得
            if let Some(cwd) = git_cwd {
                let selected = git_selected;
                let task = cx
                    .background_executor()
                    .spawn(async move { fetch_git_data(&cwd, selected.as_deref()) });
                let data = task.await;
                let ok = this.update(cx, |app: &mut TakoApp, cx| {
                    app.git_data = data;
                    cx.notify();
                });
                if ok.is_err() {
                    break;
                }
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
                        let osc = session.and_then(|s| s.title());
                        let detail_title = osc
                            .filter(|s| !s.is_empty())
                            .or(p.title())
                            .unwrap_or("")
                            .to_string();
                        AgentEntry {
                            pane: p.id(),
                            label,
                            state: session
                                .map(|s| s.command_state())
                                .unwrap_or(CommandState::Unknown),
                            backend: self.backend_sessions.get(&p.id()).cloned(),
                            detail_title,
                        }
                    })
                    .collect();
                rows.sort_by_key(|e| state_rank(e.state));
                let backgrounded = self.background_entries_of_tab(tab.id());
                TmuxViewTabGroup {
                    tab: tab.id(),
                    title: tab.title().to_string(),
                    rows,
                    sessions: self.tmux_sessions_attached_to(tab.id().as_u64()),
                    backgrounded,
                }
            })
            .collect()
    }

    /// バックグラウンドペインの表示ラベル（title > role > cwd ベース名 > 既定）。
    /// タブツリーのバックグラウンド行とドロワーのカードで共用する
    fn background_label(&self, p: &tako_core::BackgroundPane) -> String {
        p.title()
            .map(|s| s.to_string())
            .or_else(|| p.role().map(|s| s.to_string()))
            .or_else(|| {
                // cwd のベース名（例: ~/projects/tako → 「tako」）で意味づけする
                self.terminals
                    .get(&p.id())
                    .and_then(|s| s.cwd())
                    .and_then(|c| c.file_name())
                    .map(|n| format!("ターミナル: {}", n.to_string_lossy()))
            })
            .unwrap_or_else(|| "ターミナル".to_string())
    }

    /// バックグラウンドペインのコマンド状態（ターミナル不在なら Unknown）
    fn background_state(&self, pane_id: PaneId) -> CommandState {
        self.terminals
            .get(&pane_id)
            .map(|s| s.command_state())
            .unwrap_or(CommandState::Unknown)
    }

    /// 指定した由来タブのバックグラウンドペインのエントリ列（FR-2.15.6。タブ別分離表示用）
    fn background_entries_of_tab(&self, origin: TabId) -> Vec<BackgroundEntry> {
        self.workspace
            .shelved_panes()
            .iter()
            .filter(|p| p.origin_tab() == origin)
            .map(|p| BackgroundEntry {
                pane: p.id(),
                label: self.background_label(p),
                state: self.background_state(p.id()),
            })
            .collect()
    }

    /// 由来タブが既に閉じているバックグラウンドペインを、由来タブごとにまとめて返す（FR-2.15.6）。
    /// 生存タブのバックグラウンドは各タブ枠（`tmux_view_groups` の `shelved`）が表示するためここでは除く
    fn tmux_view_closed_origin_background(&self) -> Vec<ClosedOriginBackgroundGroup> {
        use std::collections::hash_map::Entry;
        let mut groups: Vec<ClosedOriginBackgroundGroup> = Vec::new();
        // 由来タブ ID → groups 内の位置（初出順を保つ）
        let mut index: std::collections::HashMap<TabId, usize> = std::collections::HashMap::new();
        for p in self.workspace.shelved_panes() {
            let origin = p.origin_tab();
            // 生存タブ由来は各タブ枠で表示済みなので除外する
            if self.workspace.get_tab(origin).is_some() {
                continue;
            }
            let entry = BackgroundEntry {
                pane: p.id(),
                label: self.background_label(p),
                state: self.background_state(p.id()),
            };
            let next = groups.len();
            match index.entry(origin) {
                Entry::Occupied(e) => groups[*e.get()].entries.push(entry),
                Entry::Vacant(e) => {
                    e.insert(next);
                    groups.push(ClosedOriginBackgroundGroup {
                        tab: origin,
                        title: p.origin_tab_title().to_string(),
                        entries: vec![entry],
                    });
                }
            }
        }
        groups
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
        // バックグラウンド中ペインの backend セッションは「バックグラウンド中」セクションで表示するため、
        // ここ（kill漏れ?/管理外）からは除外する（二重表示の防止。2026-06-15）
        let bg_sessions: std::collections::HashSet<String> = self
            .workspace
            .shelved_panes()
            .iter()
            .filter_map(|p| self.backend_sessions.get(&p.id()).cloned())
            .collect();
        self.tmux_sessions
            .iter()
            .filter_map(|session| {
                let backend = session["backend"].as_bool().unwrap_or(false);
                if backend && session["backend_pane"].as_u64().is_some() {
                    return None; // タブ枠内のペイン行で表示済み
                }
                if let Some(name) = session["name"].as_str() {
                    if name.starts_with("tako-view-") {
                        return None; // tako 内部の viewer セッション（管理外に出さない）
                    }
                    if bg_sessions.contains(name) {
                        return None; // バックグラウンド中セクションで表示するため除外
                    }
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
        // ジャンプ後はホバープレビューを畳む（対象が前面化すると hover-leave が
        // 発火せずポップアップが残るため。FR-2.16.13）
        self.hover_preview = None;
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

    /// tmux ビューのタブ枠折りたたみを設定する（FR-2.16.14）。collapsed 省略時はトグル。
    /// UI の逆三角トグルと dispatch（CLI / MCP）の両方から呼ぶ（操作経路を一本化）
    fn set_tmux_collapsed(&mut self, tab: TabId, collapsed: Option<bool>) {
        let now = collapsed.unwrap_or_else(|| !self.collapsed_tmux_tabs.contains(&tab));
        if now {
            self.collapsed_tmux_tabs.insert(tab);
        } else {
            self.collapsed_tmux_tabs.remove(&tab);
        }
    }

    /// プレビューのピン留めを設定する（FR-2.16.15）。pinned 省略時はトグル。
    /// UI の 📌 ボタンと dispatch（CLI / MCP）の両方から呼ぶ（操作経路を一本化）。
    /// 新規ピンは重ならないようカスケード配置する（以後 D&D で動かせる）
    fn set_pin(&mut self, target: PreviewTarget, pinned: Option<bool>) {
        let existing = self.pinned_previews.iter().position(|p| p.target == target);
        let want = pinned.unwrap_or(existing.is_none());
        match (want, existing) {
            (true, None) => {
                let n = self.pinned_previews.len() as f32;
                self.pinned_previews.push(PinnedPreview {
                    target,
                    pos: point(px(160.0 + n * 28.0), px(120.0 + n * 28.0)),
                });
            }
            (false, Some(i)) => {
                self.pinned_previews.remove(i);
            }
            _ => {}
        }
    }

    /// tmux 一覧を更新する。UI も CLI / MCP と同じコマンド層（dispatch）を通す
    fn refresh_tmux(&mut self, cx: &mut Context<Self>) {
        self.refresh_tmux_data();
        cx.notify();
    }

    /// tmux 一覧の取得だけ（再描画通知なし。dispatch 内から呼べる形。
    /// CLI / MCP 経由の同期更新と、パネルタブ切替時の即時更新に使う）
    fn refresh_tmux_data(&mut self) {
        let value = tako_control::dispatch(
            self,
            tako_control::protocol::Request::TmuxList { socket: None },
            PaneOrigin::User,
        )
        .unwrap_or_else(|_| serde_json::json!({ "sessions": [] }));
        self.tmux_sessions = value["sessions"].as_array().cloned().unwrap_or_default();
        self.sync_backend_windows();
    }

    /// tmux_sessions JSON からバックエンドペインの window 一覧を抽出する。
    /// 2+ window のセッションのみ backend_windows に保持し、非アクティブ window の
    /// テキストを capture_pane で取得して window_captures にキャッシュする
    fn sync_backend_windows(&mut self) {
        self.backend_windows.clear();
        let socket = tako_core::tmux_backend::socket_name();
        for (pane_id, session_name) in &self.backend_sessions {
            let session = self.tmux_sessions.iter().find(|s| {
                s["name"].as_str() == Some(session_name) && s["backend"].as_bool() == Some(true)
            });
            let Some(session) = session else { continue };
            let windows: Vec<tako_core::TmuxWindow> = session["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|w| {
                    Some(tako_core::TmuxWindow {
                        index: w["index"].as_u64()? as u32,
                        name: w["name"].as_str()?.to_string(),
                        active: w["active"].as_bool().unwrap_or(false),
                        panes: w["panes"].as_u64().unwrap_or(1) as u32,
                    })
                })
                .collect();
            if windows.len() > 1 {
                for w in &windows {
                    if !w.active {
                        let lines = tako_core::tmux::capture_pane_text(
                            Some(&socket),
                            session_name,
                            w.index,
                        );
                        self.window_captures.insert((*pane_id, w.index), lines);
                    }
                }
                self.backend_windows.insert(*pane_id, windows);
            }
        }
        // 古いキャプチャを掃除する（対応するペインや window がなくなった分）
        self.window_captures.retain(|(pane, win), _| {
            self.backend_windows
                .get(pane)
                .map(|ws| ws.iter().any(|w| w.index == *win))
                .unwrap_or(false)
        });
    }

    /// アクティブタブの最初のペインの cwd を返す（git パネルの cwd 連動用）
    fn active_tab_cwd(&self) -> Option<std::path::PathBuf> {
        let tab = self.workspace.active_tab();
        let active_pane = tab.tree().focused();
        self.terminals
            .get(&active_pane)
            .and_then(|s| s.cwd())
            .map(|p| p.to_path_buf())
            .or_else(|| {
                tab.tree().panes().iter().find_map(|p| {
                    self.terminals
                        .get(&p.id())
                        .and_then(|s| s.cwd())
                        .map(|p| p.to_path_buf())
                })
            })
    }

    /// background thread で tmux セッション一覧を取得するためのコンテキストを収集する。
    /// UI スレッドでの実行コスト: pane/tab の走査のみ（< 0.1ms）
    /// alt_screen 遷移待ちの遅延書き込みを flush する（汎用）。
    /// 対象ペインが alt_screen に入っていれば書き込み、まだなら保留。60 秒超で破棄
    fn flush_alt_screen_writes(&mut self) {
        let mut remaining = Vec::new();
        for (pane, data, created_at) in std::mem::take(&mut self.alt_screen_writes) {
            if created_at.elapsed() > std::time::Duration::from_secs(60) {
                continue;
            }
            if let Some(session) = self.terminals.get(&pane) {
                if session.is_alt_screen() {
                    session.write(data);
                } else {
                    remaining.push((pane, data, created_at));
                }
            } else {
                remaining.push((pane, data, created_at));
            }
        }
        self.alt_screen_writes = remaining;
    }

    /// claude TUI へのプロンプト送信フローを駆動する。
    /// 画面内容を確認しながら各ステップを進める（sleep ベースではない）
    fn drive_prompt_flows(&mut self) {
        let mut remaining = Vec::new();
        let now = std::time::Instant::now();
        for mut flow in std::mem::take(&mut self.prompt_flows) {
            if flow.created_at.elapsed() > std::time::Duration::from_secs(120) {
                continue;
            }
            let session = match self.terminals.get(&flow.pane) {
                Some(s) => s,
                None => {
                    remaining.push(flow);
                    continue;
                }
            };
            match flow.state {
                PromptFlowState::WaitAltScreen => {
                    if session.is_alt_screen() {
                        flow.state = PromptFlowState::WaitPromptReady;
                        flow.state_entered_at = now;
                    }
                    remaining.push(flow);
                }
                PromptFlowState::WaitPromptReady => {
                    let lines = session.visible_lines();
                    let has_prompt = lines.iter().any(|l| l.contains('❯'));
                    if has_prompt {
                        session.write(flow.prompt.as_bytes().to_vec());
                        flow.state = PromptFlowState::WaitTextEchoed;
                        flow.state_entered_at = now;
                    }
                    remaining.push(flow);
                }
                PromptFlowState::WaitTextEchoed => {
                    let lines = session.visible_lines();
                    let prefix: String = flow.prompt.chars().take(10).collect();
                    let echoed = lines.iter().any(|l| l.contains(&prefix));
                    // テキスト確認 or 10秒タイムアウトで Enter を送信。
                    // 長いプロンプトの折り返しや日本語テキストの幅計算でマッチしない
                    // ケースを救済する
                    let timed_out =
                        flow.state_entered_at.elapsed() > std::time::Duration::from_secs(10);
                    if echoed || timed_out {
                        session.write(b"\r".to_vec());
                        flow.state = PromptFlowState::WaitProcessing;
                        flow.state_entered_at = now;
                    }
                    remaining.push(flow);
                }
                PromptFlowState::WaitProcessing => {
                    flow.state = PromptFlowState::Done;
                }
                PromptFlowState::Done => {}
            }
        }
        self.prompt_flows = remaining;
    }

    fn collect_tmux_context(&self) -> tako_control::TmuxContext {
        let ws = &self.workspace;
        let pane_of_tty: Vec<(String, u64, u64)> = ws
            .tabs()
            .iter()
            .flat_map(|tab| {
                tab.tree().panes().into_iter().filter_map(|p| {
                    let tty = self.terminals.get(&p.id())?.tty_name()?;
                    Some((tty.to_string(), p.id().as_u64(), tab.id().as_u64()))
                })
            })
            .collect();
        let backend_of: Vec<(String, u64, u64)> = ws
            .tabs()
            .iter()
            .flat_map(|tab| {
                tab.tree().panes().into_iter().filter_map(|p| {
                    let name = self.backend_sessions.get(&p.id())?.clone();
                    Some((name, p.id().as_u64(), tab.id().as_u64()))
                })
            })
            .collect();
        tako_control::TmuxContext {
            pane_of_tty,
            backend_of,
        }
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
                socket: socket.clone(),
                session: session.clone(),
                window,
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: tmux を kill できない: {e}");
        }
        // セッション kill 時、そのセッションを TmuxOpen で表示していたペインを閉じる
        if window.is_none() {
            self.close_tmux_view_panes(&session, socket.as_deref(), cx);
        }
        self.refresh_tmux(cx);
    }

    /// 指定セッションを TmuxOpen で表示していたペインをすべて閉じる
    fn close_tmux_view_panes(
        &mut self,
        session: &str,
        socket: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let panes: Vec<PaneId> = self
            .tmux_view_panes
            .iter()
            .filter(|(_, target)| target.session == session && target.socket.as_deref() == socket)
            .map(|(id, _)| *id)
            .collect();
        for pane_id in panes {
            self.remove_pane(pane_id, cx);
        }
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

    /// フォーカス中ペインのフォントサイズを delta 分だけ変更する
    fn zoom_focused_pane(&mut self, delta: f32, cx: &mut Context<Self>) {
        let pane_id = self.focused_pane();
        let current = self.pane_font_size(pane_id);
        let new_size = (current + delta).clamp(Self::FONT_SIZE_MIN, Self::FONT_SIZE_MAX);
        if (new_size - current).abs() < 0.01 {
            return;
        }
        self.pane_font_sizes.insert(pane_id, new_size);
        self.pane_cell_sizes.remove(&pane_id);
        cx.notify();
    }

    /// フォーカス中ペインのフォントサイズをテーマ既定に戻す
    fn reset_zoom_focused_pane(&mut self, cx: &mut Context<Self>) {
        let pane_id = self.focused_pane();
        self.pane_font_sizes.remove(&pane_id);
        self.pane_cell_sizes.remove(&pane_id);
        cx.notify();
    }

    /// ペインの × ボタン（FR-1.3 の補助 UI）。CLI / MCP と同じコマンド層（dispatch）を
    /// 通す（開発不変条件の UI 側の一貫性）。「最後のタブの最後の 1 ペイン」は dispatch が
    /// 拒否するため、誤クリックでアプリが終了することはない（終了は cmd+W / cmd+Q のみ）
    /// ペインタイトルバーの split ボタン: 指定ペインの右に新しいペインを分割する
    fn split_pane_button(
        &mut self,
        pane_id: PaneId,
        direction: SplitDirection,
        cx: &mut Context<Self>,
    ) {
        let options = SpawnOptions {
            cwd: self
                .terminals
                .get(&pane_id)
                .and_then(|s| s.cwd())
                .filter(|p| p.is_dir())
                .map(|p| p.to_path_buf()),
            ..SpawnOptions::default()
        };
        let pane = Pane::new(PaneOrigin::User);
        let new_id = pane.id();
        if self
            .workspace
            .active_tab_mut()
            .tree_mut()
            .split(pane_id, direction, pane)
            .is_ok()
        {
            if let Err(e) = self.spawn_session(new_id, options, cx) {
                eprintln!("warning: ペインを開けない: {e}");
                self.remove_pane(new_id, cx);
            }
        }
        cx.notify();
    }

    /// ペインタイトルバーの × ボタン = ペインを閉じる（kill）。タブの × と挙動を統一する。
    /// 紐づく tmux セッション（backend の tako-* / view の tako-view-* ラッパー）を
    /// `remove_pane` が確実に kill するため、管理外 / orphan として残らない。
    /// 「閉じたいが処理は生かしたい」ときはタイトルバーの ー ボタン（`background_pane_button`）を使う
    fn close_pane_button(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        self.remove_pane(pane_id, cx);
    }

    /// ペインタイトルバーの ー ボタン = ペインをバックグラウンドへバックグラウンド（FR-2.15.1）。
    /// プロセス・tmux セッションは生かしたまま、ツリーから外してバックグラウンドに移す。
    /// 最後のタブの最後のペインのときは代替ペインを生やしてからバックグラウンドする
    fn background_pane_button(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        match self.workspace.shelve_pane(pane_id) {
            Ok(()) => {}
            Err(WorkspaceError::LastTab) => {
                // 最後のタブの最後のペイン: 新しいペインを生やしてからリトライ
                let new_pane = Pane::new(PaneOrigin::User);
                let new_id = new_pane.id();
                let focused = self.workspace.active_tab().tree().focused();
                if self
                    .workspace
                    .active_tab_mut()
                    .tree_mut()
                    .split(focused, SplitDirection::Right, new_pane)
                    .is_ok()
                {
                    if let Err(e) = self.spawn_session(new_id, SpawnOptions::default(), cx) {
                        eprintln!("warning: 代替ペインを起動できない: {e}");
                    }
                    if let Err(e) = self.workspace.shelve_pane(pane_id) {
                        eprintln!("warning: バックグラウンドへバックグラウンドできない: {e}");
                    }
                }
            }
            Err(e) => eprintln!("warning: バックグラウンドへバックグラウンドできない: {e}"),
        }
        cx.notify();
    }

    /// タブ内の全ペインをバックグラウンドへバックグラウンドする（FR-2.15 タブ単位バックグラウンド）
    fn background_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        match self.workspace.shelve_tab(tab_id) {
            Ok(_shelved_ids) => {}
            Err(WorkspaceError::LastTab) => {
                let new_pane = Pane::new(PaneOrigin::User);
                let new_id = new_pane.id();
                let title = format!("{}", self.workspace.tabs().len() + 1);
                self.workspace.create_tab(title, new_pane);
                if let Err(e) = self.spawn_session(new_id, SpawnOptions::default(), cx) {
                    eprintln!("warning: 代替ペインを起動できない: {e}");
                }
                if let Err(e) = self.workspace.shelve_tab(tab_id) {
                    eprintln!("warning: タブをバックグラウンドへバックグラウンドできない: {e}");
                }
            }
            Err(e) => eprintln!("warning: タブをバックグラウンドへバックグラウンドできない: {e}"),
        }
        cx.notify();
    }

    /// ペインを閉じたときのバックエンドセッション破棄（Phase 5.5）。
    /// **明示 close のときだけ**呼ぶ（アプリ終了経路では呼ばない = セッションが残り永続化）。
    /// シェル exit 由来の close では既にセッションが消えており kill は無害な空振りになる
    fn drop_backend_session(&mut self, pane_id: PaneId) {
        if let Some(name) = self.backend_sessions.remove(&pane_id) {
            let socket = tako_core::tmux_backend::socket_name();
            std::thread::spawn(move || tako_core::tmux_backend::kill_session(&socket, &name));
        }
    }

    /// TmuxOpen で attach した外部 tmux セッションを kill する。
    /// ペイン閉じ/タブ閉じのときに呼び、orphan 化を防ぐ
    fn drop_tmux_view_session(&mut self, pane_id: PaneId) {
        if let Some(target) = self.tmux_view_panes.remove(&pane_id) {
            // 表示用ラッパー grouped session だけを kill する。直接 attach（wrapper=None）の
            // 場合は元セッション（ユーザーのもの）なので決して触らない。`tako-view-` 接頭辞の
            // 二重ガードで、万一トラッキングがずれても元セッションを誤爆しない
            let Some(wrapper) = target.wrapper else {
                return;
            };
            if !wrapper.starts_with("tako-view-") {
                return;
            }
            let socket = target.socket;
            std::thread::spawn(move || {
                let _ = tako_core::tmux::kill_session(socket.as_deref(), &wrapper);
            });
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
                self.video_players.remove(&pane_id);
                self.video_frame_cache.remove(&pane_id);
                self.video_seek_bar_bounds.remove(&pane_id);
                self.preview_selections.remove(&pane_id);
                self.preview_line_bounds.remove(&pane_id);
                self.preview_line_texts.remove(&pane_id);
                self.scroll_accum.remove(&pane_id);
                self.scroll_ctls.remove(&pane_id);
                self.pane_font_sizes.remove(&pane_id);
                self.pane_cell_sizes.remove(&pane_id);
                self.drop_tmux_view_session(pane_id);
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
                    self.video_players.remove(&id);
                    self.video_frame_cache.remove(&id);
                    self.video_seek_bar_bounds.remove(&id);
                    self.preview_selections.remove(&id);
                    self.preview_line_bounds.remove(&id);
                    self.preview_line_texts.remove(&id);
                    self.scroll_accum.remove(&id);
                    self.scroll_ctls.remove(&id);
                    self.drop_tmux_view_session(id);
                    self.drop_backend_session(id);
                }
            }
            Err(_) => {
                // LastTab: アプリ終了は UI 層の責務。最後のペインの明示 close なので
                // セッションも破棄し、次回起動で空レイアウトを復元しないようファイルも消す
                for id in pane_ids {
                    self.drop_tmux_view_session(id);
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
        let mut layout = tako_control::layout::capture(
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
        // 折りたたみ状態（FR-2.16.14）を埋める。現存タブのみ（閉じたタブの残骸は除く）
        layout.collapsed = self
            .collapsed_tmux_tabs
            .iter()
            .filter(|t| self.workspace.get_tab(**t).is_some())
            .map(|t| t.as_u64())
            .collect();
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

    fn select_all_preview(&mut self, cx: &mut Context<Self>) {
        let pane_id = self.focused_pane();
        if !self.previews.contains_key(&pane_id) {
            return;
        }
        let Some(texts) = self.preview_line_texts.get(&pane_id) else {
            return;
        };
        if texts.is_empty() {
            return;
        }
        let last_line = texts.len() - 1;
        let last_col = texts[last_line].len();
        self.preview_selections.insert(
            pane_id,
            PreviewSelection {
                anchor: (0, 0),
                head: (last_line, last_col),
            },
        );
        cx.notify();
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        // プレビューペインの選択テキストを優先
        if let Some(text) = self.preview_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            return;
        }
        if let Some(text) = self.focused_session().and_then(|s| s.selection_text()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// プレビューペインの選択テキストを抽出する
    fn preview_selected_text(&self) -> Option<String> {
        let pane_id = self.focused_pane();
        let sel = self.preview_selections.get(&pane_id)?;
        let texts = self.preview_line_texts.get(&pane_id)?;
        if texts.is_empty() {
            return None;
        }
        let ((sl, sc), (el, ec)) = sel.ordered();
        if sl == el {
            let line = texts.get(sl)?;
            let sc = snap_to_char_boundary(line, sc.min(line.len()));
            let ec = snap_to_char_boundary(line, ec.min(line.len()));
            if sc >= ec {
                return None;
            }
            return Some(line[sc..ec].to_string());
        }
        let mut result = String::new();
        for i in sl..=el.min(texts.len() - 1) {
            let line = &texts[i];
            if i == sl {
                let sc = snap_to_char_boundary(line, sc.min(line.len()));
                result.push_str(&line[sc..]);
            } else if i == el {
                let ec = snap_to_char_boundary(line, ec.min(line.len()));
                result.push_str(&line[..ec]);
            } else {
                result.push_str(line);
            }
            if i < el.min(texts.len() - 1) {
                result.push('\n');
            }
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn preview_hit_test(&self, pane_id: PaneId, position: Point<Pixels>) -> Option<(usize, usize)> {
        let bounds_list = self.preview_line_bounds.get(&pane_id)?;
        let texts = self.preview_line_texts.get(&pane_id)?;
        let cell_w = self.cell_size.map(|c| c.width).unwrap_or(px(8.0));
        for (i, b) in bounds_list.iter().enumerate() {
            if position.y >= b.top() && position.y < b.bottom() {
                let x_offset = (position.x - b.left()).max(px(0.0));
                let char_col = (x_offset / cell_w).floor() as usize;
                let line_text = texts.get(i).map(|t| t.as_str()).unwrap_or("");
                let byte_offset = line_text
                    .char_indices()
                    .nth(char_col)
                    .map(|(pos, _)| pos)
                    .unwrap_or(line_text.len());
                return Some((i, byte_offset));
            }
        }
        texts.last().map(|last| (texts.len() - 1, last.len()))
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
        if self.inline_edit.is_some() {
            self.handle_inline_edit_key(keystroke, cx);
            cx.stop_propagation();
            return;
        }
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
    fn pane_cursor_origin(&self, pane: PaneId, _window: &mut Window) -> Option<Point<Pixels>> {
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane)?;
        let cell = self.cell_size_for_pane(pane)?;
        let screen = self.terminals.get(&pane)?.screen(&self.theme);
        let (col, row) = screen.cursor?;
        // ターミナルはセルグリッドなので x = col * cell_width が正。
        // 旧実装は shape_line().x_for_index() でフォント advance から求めていたが、
        // 全角文字の advance ≠ 2*cell_width でカーソルがずれる原因だった
        let x = f32::from(cell.width) * col as f32;
        Some(point(
            area.origin.x + px(x),
            area.origin.y + cell.height * row as f32,
        ))
    }

    /// 未確定文字列の先頭から指定プレフィックスまでの描画幅（候補ウィンドウの位置出し用）
    fn ime_prefix_width(&self, prefix: &str, pane: PaneId, window: &mut Window) -> Pixels {
        if prefix.is_empty() {
            return px(0.0);
        }
        let fs = self.pane_font_size(pane);
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
            .shape_line(SharedString::from(prefix.to_string()), px(fs), &[run], None)
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
        _window: &mut Window,
    ) -> Option<(usize, usize, bool)> {
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane_id)?;
        let cell = self.cell_size_for_pane(pane_id)?;
        let session = self.terminals.get(&pane_id)?;
        let (cols, rows) = session.size();
        let local = position - area.origin;
        let y = (f32::from(local.y) / f32::from(cell.height)).max(0.0);
        let row = (y as usize).min(rows.saturating_sub(1));
        let local_x = f32::from(local.x).max(0.0);
        let cw = f32::from(cell.width);

        // ターミナルはセルグリッドなので x / cell_width で列が決まる。
        // 旧実装は shape_line で文字位置を求めていたが、全角文字の advance ≠ 2*cell_width
        // で描画位置とクリック判定がずれる原因だった
        let col = ((local_x / cw) as usize).min(cols.saturating_sub(1));
        let cell_x = col as f32 * cw;
        let side_right = (local_x - cell_x) / cw > 0.5;
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
            .text_color(hsla(theme.red))
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
                            .bg(rgba_alpha(theme.red, 0.25))
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

    /// 表示分類バッジ（FR-2.16.12）。前面表示中 = アクティブタブ所属、それ以外は裏で実行中。
    /// タブツリーのペイン行・バックグラウンド行で共用する
    /// attach 中の外部 tmux セッションをホストペイン配下に入れ子表示する（FR-2.16.6 一本化 /
    /// FR-2.16.9）。ホスト行の下にインデントして「セッション名 + window 一覧 + 確認つき kill」を
    /// 描く。どのペインが attach しているかはホスト行が示すので「ペイン N で attach 中」は省く
    fn render_attached_session_rows(
        &self,
        group_index: usize,
        s_index: usize,
        session: &AttachedTmuxSession,
        pending_tmux: &Option<(String, Option<u32>, Option<String>)>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = &self.theme;
        // 確認 UI の id 衝突を避ける（ペイン kill は pane id、こちらは上位ビット）
        let id_seed = (1 << 32) | ((group_index as u64) << 16) | s_index as u64;
        let kill_name = session.name.clone();
        let kill_socket = session.socket.clone();
        let mut container = div().flex().flex_col().gap_1().pl_4().child(
            div()
                .id(("tmux-att-row", id_seed))
                .group("tmux-att-row")
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_1()
                .overflow_hidden()
                .cursor(CursorStyle::OpenHand)
                // D&D でタブ内へ取り込み（FR-2.16.10。attach 済みでも多重 attach 可）
                .on_drag(
                    TmuxSessionDrag {
                        name: session.name.clone(),
                        socket: session.socket.clone(),
                        window: None,
                    },
                    self.drag_ghost_builder(
                        DragKind::TmuxSession,
                        format!("tmux: {}", truncate(&session.name, 24)),
                        cx,
                    ),
                )
                .child(
                    div()
                        .px_1()
                        .flex_none()
                        .rounded_sm()
                        .text_size(px(10.0))
                        .text_color(hsla(theme.accent))
                        .bg(rgba_alpha(theme.accent, 0.15))
                        .child("⎇ tmux"),
                )
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_weight(FontWeight::BOLD)
                        .text_size(px(11.0))
                        .child(SharedString::from(truncate(&session.name, 24))),
                )
                .child(
                    div()
                        .id(("tmux-att-kill", id_seed))
                        .px_1()
                        .flex_none()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(hsla_alpha(theme.red, 0.8))
                        .opacity(0.0)
                        .group_hover("tmux-att-row", |d| d.opacity(1.0))
                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.tmux_pending_kill =
                                Some((kill_name.clone(), None, kill_socket.clone()));
                            cx.notify();
                        }))
                        .child("×"),
                ),
        );
        for (w_index, label) in &session.windows {
            let w_index = *w_index;
            let kill_name = session.name.clone();
            let kill_socket = session.socket.clone();
            let drag_name = session.name.clone();
            let drag_socket = session.socket.clone();
            container = container.child(
                div()
                    .id((
                        "tmux-att-window-row",
                        (id_seed << 8) | w_index as u64 | 0x8000_0000,
                    ))
                    .group("tmux-att-wrow")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .pl_4()
                    .text_size(px(11.0))
                    .cursor(CursorStyle::OpenHand)
                    .on_drag(
                        TmuxSessionDrag {
                            name: drag_name,
                            socket: drag_socket,
                            window: Some(w_index),
                        },
                        self.drag_ghost_builder(
                            DragKind::TmuxSession,
                            format!("tmux: {}", truncate(label, 24)),
                            cx,
                        ),
                    )
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
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
                            .text_color(hsla_alpha(theme.red, 0.8))
                            .opacity(0.0)
                            .group_hover("tmux-att-wrow", |d| d.opacity(1.0))
                            .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tmux_pending_kill =
                                    Some((kill_name.clone(), Some(w_index), kill_socket.clone()));
                                cx.notify();
                            }))
                            .child("🗑"),
                    ),
            );
        }
        // attach 済みセッションへの kill 確認（unlisted 側と同じ pending を使う）
        if let Some((pending_session, pending_window, _)) = pending_tmux {
            if *pending_session == session.name {
                let label = match pending_window {
                    Some(w) => {
                        format!("window {w} を kill していいですか?（中のプロセスごと終了）")
                    }
                    None => format!(
                        "セッション {} を kill していいですか?（中のプロセスごと終了。\
                         attach 中のペインからも消える）",
                        session.name
                    ),
                };
                container = container.child(self.render_kill_confirm(id_seed, label, None, cx));
            }
        }
        container
    }

    /// バックグラウンドペインのバックグラウンド行（FR-2.15.6）。タブ枠内（タブ別分離）と
    /// 「閉じたタブ」グループで共用。バッジ + 状態ドット + ラベル + 復帰（由来タブへ戻す）。
    /// D&D でもペインエリアへ復帰できる（ドロワーと同じ BackgroundPaneDrag）
    fn render_background_row(
        &self,
        entry: &BackgroundEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = &self.theme;
        let pane_id = entry.pane;
        let state_color = match entry.state {
            CommandState::Failed(_) => Some(theme.red),
            CommandState::Running => Some(theme.accent),
            CommandState::Idle => Some(theme.yellow),
            _ => None,
        };
        let mut row = div()
            .id(("tmux-bg-row", pane_id.as_u64()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py(px(2.0))
            .rounded_sm()
            .border_1()
            .border_color(hsla(theme.border_heavy))
            .bg(rgba(tako_core::Rgb::from_hex(0x161620)))
            .text_size(px(11.0))
            .text_color(hsla(theme.tab_inactive_foreground))
            .hover(|d| d.border_color(hsla(theme.text_overlay)))
            .cursor(CursorStyle::OpenHand)
            .on_drag(
                BackgroundPaneDrag { pane: pane_id },
                self.drag_ghost_builder(DragKind::BackgroundPane, truncate(&entry.label, 24), cx),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(hsla(theme.text_faint))
                    .child("⠿"),
            );
        if let Some(color) = state_color {
            row = row.child(
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .flex_none()
                    .rounded_full()
                    .bg(hsla(color)),
            );
        }
        row.child(
            div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(SharedString::from(format!(
                    "{}（BG）",
                    truncate(&entry.label, 22)
                ))),
        )
        .child(
            div()
                .id(("tmux-bg-restore", pane_id.as_u64()))
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(10.0))
                .text_color(hsla(theme.accent))
                .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                .child("⬆")
                .on_click(cx.listener(move |this, _, _, cx| {
                    // 由来タブが生きていればそこへ、無ければアクティブタブへ戻す
                    let origin = this.workspace.shelved_origin_tab(pane_id);
                    let target = origin
                        .and_then(|t| this.workspace.get_tab(t))
                        .map(|t| t.tree().focused())
                        .unwrap_or_else(|| this.workspace.active_tab().tree().focused());
                    if let Err(e) =
                        this.workspace
                            .unshelve_pane(pane_id, target, SplitDirection::Right)
                    {
                        eprintln!("warning: バックグラウンドから復帰できない: {e}");
                    }
                    if this.workspace.shelved_panes().is_empty() {
                        this.drawer_visible = false;
                    }
                    cx.notify();
                })),
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
            .gap(px(6.0))
            .p(px(8.0))
            .bg(rgba(theme.mantle))
            .text_color(hsla(theme.foreground))
            .text_size(px(12.0))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child("WORKSPACE"),
                    )
                    .child(div().flex_grow(1.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(hsla(theme.accent)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(hsla(theme.tab_inactive_foreground))
                                            .child("run"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(hsla(theme.red)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(hsla(theme.tab_inactive_foreground))
                                            .child("fail"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .bg(hsla(theme.green)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(hsla(theme.tab_inactive_foreground))
                                            .child("idle"),
                                    ),
                            ),
                    ),
            );

        // タブ枠: タブ名ラベル付き四角枠 + 枠内に全ペインの入れ子表示（FR-2.16.6）
        for (group_index, group) in groups.into_iter().enumerate() {
            let is_active = group.tab == active_tab;
            let tab_id = group.tab;
            let is_collapsed = self.collapsed_tmux_tabs.contains(&tab_id);
            // 折りたたみ時はバックグラウンド項目（裏で実行中の行 + バックグラウンド）を隠し、前面表示中
            // （アクティブタブ）の行は残す（FR-2.16.14。Q2 = バックグラウンド行＋バックグラウンドだけ隠す）。
            // タブ内の行は surface が一律（アクティブ＝全 foreground / 非アクティブ＝全 background）
            let show_rows = is_active || !is_collapsed;
            let total_pane_count = group.rows.len();

            let tab_tree = self
                .workspace
                .tabs()
                .iter()
                .find(|t| t.id() == tab_id)
                .map(|t| t.tree());
            let tab_focused = tab_tree.map(|t| t.focused());
            // レイアウト順のペイン ID リスト（ミニマップとペインリストで番号を統一するため）
            let layout_order: Vec<PaneId> = tab_tree
                .map(|t| {
                    t.layout(tako_core::Rect::UNIT)
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect()
                })
                .unwrap_or_default();
            let has_failure = group
                .rows
                .iter()
                .any(|r| matches!(r.state, CommandState::Failed(_)));
            let fail_count = group
                .rows
                .iter()
                .filter(|r| matches!(r.state, CommandState::Failed(_)))
                .count();
            let mut card = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .p(px(8.0))
                .rounded(px(9.0))
                .border_1()
                .border_color(hsla(if is_collapsed && has_failure {
                    tako_core::Rgb::from_hex(0x3a2b35)
                } else {
                    theme.border_strong
                }))
                .bg(rgba(if is_active {
                    theme.surface_1
                } else if is_collapsed && has_failure {
                    tako_core::Rgb::from_hex(0x1f1a22)
                } else if is_collapsed {
                    tako_core::Rgb::from_hex(0x1a1b27)
                } else {
                    theme.surface_0
                }))
                .when(is_active, |d| {
                    d.shadow(vec![BoxShadow {
                        color: hsla_alpha(theme.accent, 0.18),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                        inset: true,
                    }])
                })
                .child({
                    let tab_agg_color = if has_failure {
                        theme.red
                    } else if group
                        .rows
                        .iter()
                        .any(|r| matches!(r.state, CommandState::Running))
                    {
                        theme.accent
                    } else if group
                        .rows
                        .iter()
                        .any(|r| matches!(r.state, CommandState::Idle))
                    {
                        theme.green
                    } else {
                        theme.tab_inactive_foreground
                    };
                    div()
                        .id(("tmux-tab-header", tab_id.as_u64()))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .cursor_pointer()
                        .when(!is_collapsed, |d| {
                            d.bg(rgba_alpha(theme.accent, 0.08))
                                .rounded(px(4.0))
                                .px(px(4.0))
                                .py(px(2.0))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_tmux_collapsed(tab_id, None);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(if is_collapsed { "▸" } else { "▾" }),
                        )
                        .child(
                            div()
                                .w(px(7.0))
                                .h(px(7.0))
                                .flex_none()
                                .rounded_full()
                                .bg(hsla(tab_agg_color))
                                .shadow(vec![BoxShadow {
                                    color: hsla_alpha(tab_agg_color, 0.4),
                                    offset: point(px(0.), px(0.)),
                                    blur_radius: px(3.0),
                                    spread_radius: px(0.),
                                    inset: false,
                                }]),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(if is_active {
                                    hsla(theme.tab_active_foreground)
                                } else {
                                    hsla(theme.text_secondary)
                                })
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(truncate(&group.title, 28))),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(SharedString::from(format!("{total_pane_count}"))),
                        )
                        .when(is_active, |d| {
                            d.child(
                                div()
                                    .text_size(px(9.5))
                                    .font_weight(FontWeight::BOLD)
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .text_color(hsla(theme.accent))
                                    .bg(rgba_alpha(theme.accent, 0.14))
                                    .child("ACTIVE"),
                            )
                        })
                        // 折りたたみ時: インラインステートチップ（各ペインの状態を小矩形で表示）
                        .when(is_collapsed && !is_active, |d| {
                            let mut chips = div().flex().flex_row().items_center().gap(px(2.0));
                            for row in &group.rows {
                                let chip_color = match row.state {
                                    CommandState::Failed(_) => theme.red,
                                    CommandState::Running => theme.accent,
                                    CommandState::Idle => theme.green,
                                    CommandState::Unknown => theme.tab_inactive_foreground,
                                };
                                chips = chips.child(
                                    div()
                                        .w(px(8.0))
                                        .h(px(4.0))
                                        .rounded(px(1.0))
                                        .bg(hsla(chip_color)),
                                );
                            }
                            d.child(chips)
                        })
                        // 折りたたみ + fail あり: "N fail" ラベル
                        .when(is_collapsed && fail_count > 0, |d| {
                            d.child(
                                div()
                                    .text_size(px(9.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(hsla(theme.red))
                                    .child(SharedString::from(format!("{fail_count} fail"))),
                            )
                        })
                });
            // ミニレイアウトマップ（ペイン配置を小さな矩形で可視化）
            if show_rows {
                if let Some(tree) = tab_tree {
                    let focused_pane = tree.focused();
                    let layout = tree.layout(tako_core::Rect::new(0.0, 0.0, 92.0, 76.0));
                    let mut map = div()
                        .w(px(92.0))
                        .h(px(76.0))
                        .bg(rgba(theme.crust))
                        .border_1()
                        .border_color(hsla(theme.border_default))
                        .rounded(px(6.0))
                        .relative()
                        .overflow_hidden()
                        .mx_auto();
                    for (idx, (pane_id, rect)) in layout.iter().enumerate() {
                        let is_focused = *pane_id == focused_pane;
                        let pane_state = self
                            .terminals
                            .get(pane_id)
                            .map(|s| s.command_state())
                            .unwrap_or(CommandState::Unknown);
                        let cell_border_color = match pane_state {
                            CommandState::Failed(_) => theme.red,
                            CommandState::Running if is_focused => theme.accent,
                            _ if is_focused => theme.accent,
                            _ => theme.border_strong,
                        };
                        let pane_num = idx + 1;
                        let cell = div()
                            .absolute()
                            .left(px(rect.x + 1.0))
                            .top(px(rect.y + 1.0))
                            .w(px((rect.width - 2.0).max(4.0)))
                            .h(px((rect.height - 2.0).max(4.0)))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(hsla(cell_border_color))
                            .bg(rgba(theme.surface_1))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.0))
                            .font_family("Monaco")
                            .font_weight(FontWeight::BOLD)
                            .text_color(if is_focused {
                                hsla(theme.accent)
                            } else {
                                hsla(theme.tab_inactive_foreground)
                            })
                            .child(SharedString::from(format!("{pane_num}")));
                        let cell = if is_focused || matches!(pane_state, CommandState::Failed(_)) {
                            cell.shadow(vec![BoxShadow {
                                color: hsla_alpha(cell_border_color, 0.4),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(4.0),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                        } else {
                            cell
                        };
                        map = map.child(cell);
                    }
                    card = card.child(map);
                }
            }
            // 折りたたみ時はバックグラウンド行を描かない（空にして既存ループをそのまま流す）。
            // 前面表示中（アクティブタブ）の行は残す（FR-2.16.14）
            let group_rows = if show_rows { group.rows } else { Vec::new() };
            // どの attach セッションをホスト行の下に出したか（取りこぼし防止に使う）
            let mut rendered_sessions: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            for row in group_rows {
                let pane = row.pane;
                let pane_num = layout_order
                    .iter()
                    .position(|id| *id == pane)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let pinned = self
                    .pinned_previews
                    .iter()
                    .any(|p| p.target == PreviewTarget::Pane(pane));
                let pane_role = self
                    .workspace
                    .tabs()
                    .iter()
                    .find(|t| t.id() == tab_id)
                    .and_then(|t| t.tree().get(pane))
                    .and_then(|p| p.role())
                    .unwrap_or("")
                    .to_string();
                let show_state = !matches!(row.state, CommandState::Unknown);
                let color = match row.state {
                    CommandState::Failed(_) => theme.red,
                    CommandState::Idle => theme.green,
                    CommandState::Running => theme.accent,
                    CommandState::Unknown => theme.tab_inactive_foreground,
                };
                // このペインが attach 表示している外部セッション（あれば detail に名前を出す。
                // window 一覧はホスト行の下に入れ子表示するので二重化しない。FR-2.16.6）
                let hosted: Vec<&AttachedTmuxSession> = group
                    .sessions
                    .iter()
                    .filter(|s| s.pane == pane.as_u64())
                    .collect();
                let _detail = if !row.detail_title.is_empty() {
                    truncate(&row.detail_title, 36)
                } else if !hosted.is_empty() {
                    let names: Vec<String> = hosted.iter().map(|s| truncate(&s.name, 18)).collect();
                    format!("tmux: {}", names.join(" / "))
                } else {
                    match &row.backend {
                        Some(b) => format!("tmux: {}", truncate(b, 24)),
                        None => String::new(),
                    }
                };
                let is_pane_focused = tab_focused == Some(pane);
                card = card.child(
                    div()
                        .id(("tmux-pane-row", pane.as_u64()))
                        .group("tmux-row")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .overflow_hidden()
                        .when(is_pane_focused, |d| {
                            d.bg(rgba_alpha(theme.accent, 0.1)).shadow(vec![BoxShadow {
                                color: hsla(theme.accent),
                                offset: point(px(2.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(0.),
                                inset: true,
                            }])
                        })
                        .hover(|d| d.bg(rgba_alpha(theme.tab_bar_background, 0.8)))
                        .on_click(cx.listener(move |this, _, _, cx| this.jump_to_pane(pane, cx)))
                        // バックグラウンド行はホバーで実画面プレビューを出す（FR-2.16.13）。
                        // 前面表示中（アクティブタブ）はペインエリアで見えるので対象外
                        .when(!is_active, |d| {
                            d.on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                if *hovered {
                                    this.hover_preview = Some(HoverPreview {
                                        target: PreviewTarget::Pane(pane),
                                        anchor: window.mouse_position(),
                                    });
                                } else if matches!(
                                    this.hover_preview,
                                    Some(HoverPreview { target: PreviewTarget::Pane(p), .. })
                                        if p == pane
                                ) {
                                    this.hover_preview = None;
                                }
                                cx.notify();
                            }))
                        })
                        // ナンバーバッジ
                        .child(
                            div()
                                .w(px(16.0))
                                .h(px(16.0))
                                .flex_none()
                                .rounded(px(4.0))
                                .bg(rgba_alpha(color, 0.2))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(10.0))
                                .font_family("Monaco")
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(color))
                                .child(SharedString::from(format!("{pane_num}"))),
                        )
                        // 状態ドット（6px + pulse glow）
                        .when(show_state, |d| {
                            d.child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(hsla(color))
                                    .shadow(vec![BoxShadow {
                                        color: hsla_alpha(color, 0.4),
                                        offset: point(px(0.), px(0.)),
                                        blur_radius: px(3.0),
                                        spread_radius: px(0.),
                                        inset: false,
                                    }]),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family("Monaco")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(12.0))
                                .child(SharedString::from(truncate(&row.label, 20))),
                        )
                        // ロールタグ
                        .when(pane_role.contains("orchestrator-master"), |d| {
                            d.child(
                                div()
                                    .text_size(px(8.5))
                                    .font_weight(FontWeight::BOLD)
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .text_color(hsla(theme.accent))
                                    .bg(rgba_alpha(theme.accent, 0.14))
                                    .flex_none()
                                    .child("ORCH"),
                            )
                        })
                        .when(pane_role.contains("orchestrator-worker"), |d| {
                            d.child(
                                div()
                                    .text_size(px(8.5))
                                    .font_weight(FontWeight::BOLD)
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .text_color(hsla(theme.teal))
                                    .bg(rgba_alpha(theme.teal, 0.12))
                                    .flex_none()
                                    .child("WORK"),
                            )
                        })
                        // バックグラウンド行に 📌 ピン留めボタン（FR-2.16.15。ピン中は常時表示、
                        // 未ピンは行ホバー時のみ）。前面行はプレビュー対象外なので出さない
                        .when(!is_active, |d| {
                            d.child(
                                div()
                                    .id(("pane-pin", pane.as_u64()))
                                    .px_1()
                                    .flex_none()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .when(pinned, |d| d.text_color(hsla(theme.accent)))
                                    .when(!pinned, |d| {
                                        d.opacity(0.0).group_hover("tmux-row", |d| d.opacity(1.0))
                                    })
                                    .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_pin(PreviewTarget::Pane(pane), None);
                                        cx.notify();
                                    }))
                                    .child("📌"),
                            )
                        })
                        .child(
                            div()
                                .id(("pane-kill", pane.as_u64()))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla_alpha(theme.red, 0.8))
                                .opacity(0.0)
                                .group_hover("tmux-row", |d| d.opacity(1.0))
                                .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.pending_pane_kill = Some(pane);
                                    cx.notify();
                                }))
                                .child("×"),
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
                // バックエンドセッションに複数 window がある場合、非アクティブ window を
                // 子行として表示する（tmux window 統合）。クリックで window 切替
                if let Some(windows) = self.backend_windows.get(&pane) {
                    for w in windows {
                        if w.active {
                            continue; // アクティブ window はペイン本体が表示
                        }
                        let win_index = w.index;
                        let win_label = format!("  ↳ {}:{}", w.index, truncate(&w.name, 16));
                        let win_pane_count = w.panes;
                        let win_pinned = self
                            .pinned_previews
                            .iter()
                            .any(|p| p.target == PreviewTarget::TmuxWindow(pane, win_index));
                        card = card.child(
                            div()
                                .id(("tmux-win-row", pane.as_u64() * 100 + win_index as u64))
                                .group("tmux-win-row")
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .px_1()
                                .ml_4()
                                .rounded_sm()
                                .cursor_pointer()
                                .overflow_hidden()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .hover(|d| d.bg(rgba_alpha(theme.tab_bar_background, 0.8)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let _ = tako_control::dispatch(
                                        this,
                                        tako_control::protocol::Request::TmuxSelectWindow {
                                            pane: Some(pane.as_u64()),
                                            window: win_index,
                                        },
                                        PaneOrigin::User,
                                    );
                                    cx.notify();
                                }))
                                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                    if *hovered {
                                        this.hover_preview = Some(HoverPreview {
                                            target: PreviewTarget::TmuxWindow(pane, win_index),
                                            anchor: window.mouse_position(),
                                        });
                                    } else if matches!(
                                        this.hover_preview,
                                        Some(HoverPreview { target: PreviewTarget::TmuxWindow(p, w), .. })
                                            if p == pane && w == win_index
                                    ) {
                                        this.hover_preview = None;
                                    }
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .w(px(8.0))
                                        .h(px(8.0))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(hsla(theme.tab_inactive_foreground)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(win_label)),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .child(SharedString::from(format!("{win_pane_count} ペイン"))),
                                )
                                .child(
                                    div()
                                        .id(("win-pin", pane.as_u64() * 100 + win_index as u64))
                                        .px_1()
                                        .flex_none()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .when(win_pinned, |d| d.text_color(hsla(theme.accent)))
                                        .when(!win_pinned, |d| {
                                            d.opacity(0.0)
                                                .group_hover("tmux-win-row", |d| d.opacity(1.0))
                                        })
                                        .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.set_pin(
                                                PreviewTarget::TmuxWindow(pane, win_index),
                                                None,
                                            );
                                            cx.notify();
                                        }))
                                        .child("📌"),
                                ),
                        );
                    }
                }
                // ホストペイン配下に attach 中セッションを入れ子表示（FR-2.16.6 一本化）
                for (s_index, session) in group.sessions.iter().enumerate() {
                    if session.pane != pane.as_u64() {
                        continue;
                    }
                    rendered_sessions.insert(s_index);
                    card = card.child(self.render_attached_session_rows(
                        group_index,
                        s_index,
                        session,
                        &pending_tmux,
                        cx,
                    ));
                }
            }
            // ホストペインが行に出ていない attach セッションの取りこぼし防止（防御的に表示）。
            // 折りたたみ時（show_rows=false）は行ごと隠れているのでこれらも出さない
            for (s_index, session) in group.sessions.iter().enumerate() {
                if !show_rows || rendered_sessions.contains(&s_index) {
                    continue;
                }
                card = card.child(self.render_attached_session_rows(
                    group_index,
                    s_index,
                    session,
                    &pending_tmux,
                    cx,
                ));
            }
            // バックグラウンド/shelved エリア（スペック準拠: border-top 1px dashed #2b2c3e）
            if !is_collapsed && !group.backgrounded.is_empty() {
                let bg_count = group.backgrounded.len();
                let mut bg_section = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .mt(px(6.0))
                    .pt(px(6.0))
                    .border_t_1()
                    .border_color(hsla_alpha(theme.border_strong, 0.5))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(format!(
                                "BACKGROUND \u{00B7} {bg_count}"
                            ))),
                    );
                for entry in &group.backgrounded {
                    bg_section = bg_section.child(self.render_background_row(entry, cx));
                }
                card = card.child(bg_section);
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
                ("kill漏れ?", theme.red) // 赤: tako が起動して kill し損ねた残骸
            } else {
                ("管理外", theme.yellow) // 黄: tako の外で立てられたセッション
            };
            let kill_name = session.name.clone();
            let kill_socket = session.socket.clone();
            let mut card = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .rounded_md()
                .bg(rgba_alpha(theme.tab_bar_background, 0.6))
                .child(
                    div()
                        .id(("tmux-unlisted-row", index as u64))
                        .group("tmux-unlisted")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .overflow_hidden()
                        .cursor(CursorStyle::OpenHand)
                        // D&D で現在のタブへ取り込んで表示（FR-2.16.10。kill せず中身を確認できる）
                        .on_drag(
                            TmuxSessionDrag {
                                name: session.name.clone(),
                                socket: session.socket.clone(),
                                window: None,
                            },
                            self.drag_ghost_builder(
                                DragKind::TmuxSession,
                                format!("tmux: {}", truncate(&session.name, 24)),
                                cx,
                            ),
                        )
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
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(truncate(&session.name, 24))),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .flex_none()
                                .whitespace_nowrap()
                                .text_color(if session.attached {
                                    hsla(theme.accent)
                                } else {
                                    hsla(theme.yellow)
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
                                .whitespace_nowrap()
                                .text_ellipsis()
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
                                .text_color(hsla_alpha(theme.red, 0.8))
                                .opacity(0.0)
                                .group_hover("tmux-unlisted", |d| d.opacity(1.0))
                                .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tmux_pending_kill =
                                        Some((kill_name.clone(), None, kill_socket.clone()));
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                )
                .children(session.windows.iter().map(|(w_index, label)| {
                    let w_index = *w_index;
                    let kill_name = session.name.clone();
                    let kill_socket = session.socket.clone();
                    let drag_name = session.name.clone();
                    let drag_socket = session.socket.clone();
                    div()
                        .id((
                            "tmux-unlisted-wrow",
                            ((index as u64) << 16) | w_index as u64 | 0x8000_0000,
                        ))
                        .group("tmux-unlisted-wrow")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .pl_4()
                        .text_size(px(11.0))
                        .cursor(CursorStyle::OpenHand)
                        .on_drag(
                            TmuxSessionDrag {
                                name: drag_name,
                                socket: drag_socket,
                                window: Some(w_index),
                            },
                            self.drag_ghost_builder(
                                DragKind::TmuxSession,
                                format!("tmux: {}", truncate(label, 24)),
                                cx,
                            ),
                        )
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
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
                                .text_color(hsla_alpha(theme.red, 0.8))
                                .opacity(0.0)
                                .group_hover("tmux-unlisted-wrow", |d| d.opacity(1.0))
                                .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tmux_pending_kill = Some((
                                        kill_name.clone(),
                                        Some(w_index),
                                        kill_socket.clone(),
                                    ));
                                    cx.notify();
                                }))
                                .child("×"),
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

        // 由来タブが閉じたバックグラウンドペインは「タブ <名前>（閉じたタブ）」にまとめて表示する。
        // 生存タブ由来のバックグラウンドは各タブ枠内へバックグラウンド表示済み（FR-2.15.6 タブ別分離）
        let closed_origin = self.tmux_view_closed_origin_background();
        if !closed_origin.is_empty() {
            root = root.child(
                div()
                    .mt_2()
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .text_size(px(11.0))
                    .child("閉じたタブのターミナル（バックグラウンドで実行中）"),
            );
        }
        for shelf_group in &closed_origin {
            let group_tab = shelf_group.tab;
            let group_pinned = self
                .pinned_previews
                .iter()
                .any(|p| p.target == PreviewTarget::ClosedGroup(group_tab));
            let mut card = div()
                .id(("tmux-closed-group", group_tab.as_u64()))
                .group("tmux-closed-group")
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(hsla_alpha(theme.pane_border, 0.7))
                // グループ全体をホバーで一括プレビュー（FR-2.16.16。全バックグラウンドペインを並べて出す）
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    if *hovered {
                        this.hover_preview = Some(HoverPreview {
                            target: PreviewTarget::ClosedGroup(group_tab),
                            anchor: window.mouse_position(),
                        });
                    } else if matches!(
                        this.hover_preview,
                        Some(HoverPreview { target: PreviewTarget::ClosedGroup(t), .. })
                            if t == group_tab
                    ) {
                        this.hover_preview = None;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(format!(
                                    "タブ {}（閉じたタブ・{} 件）",
                                    truncate(&shelf_group.title, 20),
                                    shelf_group.entries.len()
                                ))),
                        )
                        // グループ全体を 📌 ピン留め（FR-2.16.15 / FR-2.16.16）
                        .child(
                            div()
                                .id(("group-pin", group_tab.as_u64()))
                                .px_1()
                                .flex_none()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .when(group_pinned, |d| d.text_color(hsla(theme.accent)))
                                .when(!group_pinned, |d| {
                                    d.opacity(0.0)
                                        .group_hover("tmux-closed-group", |d| d.opacity(1.0))
                                })
                                .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_pin(PreviewTarget::ClosedGroup(group_tab), None);
                                    cx.notify();
                                }))
                                .child("📌"),
                        ),
                );
            for entry in &shelf_group.entries {
                card = card.child(self.render_background_row(entry, cx));
            }
            root = root.child(card);
        }

        root
    }

    /// git ビュー（FR-3.6 git graph + FR-3.9 diff ビューア）。cwd 連動で 2 秒ポーリング更新。
    /// セクション: ブランチ → 変更ファイル → コミットグラフ → diff
    fn render_git_view(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let data = self.git_data.clone();
        let collapsed = self.git_collapsed.clone();

        let mut root = div()
            .id("git-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(rgba(theme.mantle))
            .text_color(hsla(theme.tab_inactive_foreground))
            .text_size(px(11.0));

        let Some(data) = data else {
            // git パネルを開いた瞬間のデータ取得（初回は即 fetch）
            if self.git_data.is_none() {
                if let Some(cwd) = self.active_tab_cwd() {
                    cx.spawn(async move |this, cx| {
                        let data = cx
                            .background_executor()
                            .spawn(async move { fetch_git_data(&cwd, None) })
                            .await;
                        let _ = this.update(cx, |app: &mut TakoApp, cx| {
                            app.git_data = data;
                            cx.notify();
                        });
                    })
                    .detach();
                }
            }
            return root.p_4().child("git リポジトリを検出中…");
        };

        let accent = theme.accent;
        let fg = theme.tab_inactive_foreground;
        let fg_active = theme.tab_active_foreground;
        let bg_hover = theme.selection_background;

        // ──── リポヘッダ ────
        let repo_name = std::path::Path::new(&data.repo_root)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| data.repo_root.clone());
        root = root.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .px_2()
                .py_1()
                .bg(rgba(theme.tab_bar_background))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(hsla(fg_active))
                        .child(format!("⎇ {}", data.branch)),
                )
                .child(
                    div()
                        .ml_2()
                        .text_size(px(10.0))
                        .text_color(hsla(fg))
                        .child(repo_name),
                )
                .when(!data.upstream.is_empty(), |d| {
                    d.child(
                        div()
                            .ml_2()
                            .text_size(px(10.0))
                            .text_color(hsla(fg))
                            .child(format!("↑ {}", data.upstream)),
                    )
                }),
        );

        // ──── ブランチ一覧セクション ────
        root = root.child(
            div()
                .id("git-branches-header")
                .flex()
                .flex_row()
                .items_center()
                .px_2()
                .py(px(3.0))
                .cursor_pointer()
                .text_size(px(10.0))
                .text_color(hsla(fg))
                .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.git_collapsed.branches = !this.git_collapsed.branches;
                    cx.notify();
                }))
                .child(if collapsed.branches { "▸" } else { "▾" })
                .child(format!(
                    " ブランチ ({})",
                    data.branches.iter().filter(|b| !b.is_remote).count()
                )),
        );
        if !collapsed.branches {
            for branch in &data.branches {
                if branch.is_remote {
                    continue;
                }
                let is_current = branch.is_current;
                root = root.child(
                    div()
                        .px_3()
                        .py(px(1.0))
                        .text_size(px(11.0))
                        .when(is_current, |d| d.text_color(hsla(accent)))
                        .when(!is_current, |d| d.text_color(hsla(fg)))
                        .child(format!(
                            "{}{}",
                            if is_current { "● " } else { "  " },
                            branch.name
                        )),
                );
            }
        }

        // ──── 変更ファイルセクション ────
        if !data.status.is_empty() {
            root = root.child(
                div()
                    .id("git-changes-header")
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_2()
                    .py(px(3.0))
                    .mt_1()
                    .cursor_pointer()
                    .text_size(px(10.0))
                    .text_color(hsla(fg))
                    .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git_collapsed.changes = !this.git_collapsed.changes;
                        cx.notify();
                    }))
                    .child(if collapsed.changes { "▸" } else { "▾" })
                    .child(format!(" 変更 ({})", data.status.len())),
            );
            if !collapsed.changes {
                for entry in &data.status {
                    let color = match (entry.index, entry.worktree) {
                        ('?', _) => theme.green,          // 緑 = untracked
                        (_, 'M') => theme.yellow,         // 黄 = modified
                        ('D', _) | (_, 'D') => theme.red, // 赤 = deleted
                        ('A', _) => theme.green,          // 緑 = added
                        _ => fg,
                    };
                    let badge = match (entry.index, entry.worktree) {
                        ('?', _) => "?",
                        (_, 'M') | ('M', _) => "M",
                        ('D', _) | (_, 'D') => "D",
                        ('A', _) => "A",
                        ('R', _) => "R",
                        _ => " ",
                    };
                    root = root.child(
                        div()
                            .flex()
                            .flex_row()
                            .px_3()
                            .py(px(1.0))
                            .text_size(px(11.0))
                            .child(
                                div()
                                    .w(px(14.0))
                                    .text_color(hsla(color))
                                    .child(badge.to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_ellipsis()
                                    .text_color(hsla(fg))
                                    .child(entry.path.clone()),
                            ),
                    );
                }
            }
        }

        // ──── コミットグラフセクション ────
        let selected_commit = self.git_selected_commit.clone();
        root = root.child(
            div()
                .id("git-commits-header")
                .flex()
                .flex_row()
                .items_center()
                .px_2()
                .py(px(3.0))
                .mt_1()
                .cursor_pointer()
                .text_size(px(10.0))
                .text_color(hsla(fg))
                .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.git_collapsed.commits = !this.git_collapsed.commits;
                    cx.notify();
                }))
                .child(if collapsed.commits { "▸" } else { "▾" })
                .child(format!(" コミット ({})", data.commits.len())),
        );
        if !collapsed.commits {
            for (i, commit) in data.commits.iter().enumerate() {
                let hash = commit.short_hash.clone();
                let full_hash = commit.hash.clone();
                let is_selected = selected_commit.as_deref() == Some(&commit.hash);
                let has_refs = !commit.refs.is_empty();

                let mut row = div()
                    .id(("git-commit", i))
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .px_2()
                    .py(px(2.0))
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(rgba_alpha(accent, 0.15)))
                    .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.git_selected_commit.as_deref() == Some(&full_hash) {
                            this.git_selected_commit = None;
                        } else {
                            this.git_selected_commit = Some(full_hash.clone());
                        }
                        // 即座に diff を取得する
                        if let Some(cwd) = this.active_tab_cwd() {
                            let selected = this.git_selected_commit.clone();
                            cx.spawn(async move |this, cx| {
                                let data = cx
                                    .background_executor()
                                    .spawn(async move { fetch_git_data(&cwd, selected.as_deref()) })
                                    .await;
                                let _ = this.update(cx, |app: &mut TakoApp, cx| {
                                    app.git_data = data;
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    }));

                // グラフ列（canvas 描画）
                let graph_w = {
                    const LANE_W: f32 = 14.0;
                    (data.graph.max_lanes as f32 * LANE_W + 4.0).max(18.0)
                };
                let graph_lines: Vec<tako_core::GraphLine> = if i < data.graph.rows.len() {
                    data.graph.rows[i].lines.clone()
                } else {
                    Vec::new()
                };
                let graph_commit_lane = if i < data.graph.rows.len() {
                    data.graph.rows[i].lane
                } else {
                    0
                };
                let graph_commit_color = if i < data.graph.rows.len() {
                    data.graph.rows[i].color_index
                } else {
                    0
                };
                row = row.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            paint_graph_row(
                                window,
                                bounds,
                                &graph_lines,
                                graph_commit_lane,
                                graph_commit_color,
                            );
                        },
                    )
                    .w(px(graph_w))
                    .flex_none(),
                );

                // コミット情報
                let mut info = div().flex_1().flex().flex_col();
                // 1行目: subject + refs
                let mut first_line = div().flex().flex_row().items_center().gap_1();
                first_line = first_line.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(hsla(fg_active))
                        .text_ellipsis()
                        .child(commit.subject.clone()),
                );
                if has_refs {
                    for r in commit.refs.split(", ") {
                        let badge_color = data
                            .graph
                            .ref_colors
                            .get(r)
                            .map(|&ci| tako_core::GRAPH_PALETTE[ci])
                            .unwrap_or(accent);
                        first_line = first_line.child(
                            div()
                                .px_1()
                                .rounded(px(3.0))
                                .text_size(px(9.0))
                                .bg(rgba_alpha(badge_color, 0.25))
                                .text_color(hsla(badge_color))
                                .child(r.to_string()),
                        );
                    }
                }
                info = info.child(first_line);
                // 2行目: hash + author + date
                info = info.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .text_size(px(9.0))
                        .text_color(hsla(fg))
                        .child(hash)
                        .child(commit.author.clone())
                        .child(commit.date_relative.clone()),
                );
                row = row.child(info);
                root = root.child(row);
            }
        }

        // ──── diff セクション ────
        if !data.diff_files.is_empty() {
            root = root.child(
                div()
                    .id("git-diff-header")
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_2()
                    .py(px(3.0))
                    .mt_1()
                    .cursor_pointer()
                    .text_size(px(10.0))
                    .text_color(hsla(fg))
                    .hover(|d| d.bg(rgba_alpha(bg_hover, 0.3)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git_collapsed.diff = !this.git_collapsed.diff;
                        cx.notify();
                    }))
                    .child(if collapsed.diff { "▸" } else { "▾" })
                    .child(format!(
                        " diff ({}{})",
                        data.diff_files.len(),
                        if selected_commit.is_some() {
                            " コミット"
                        } else {
                            " ファイル"
                        }
                    )),
            );
            if !collapsed.diff {
                for file in &data.diff_files {
                    // ファイルヘッダ
                    root = root.child(
                        div()
                            .px_3()
                            .py(px(2.0))
                            .text_size(px(10.0))
                            .text_color(hsla(fg_active))
                            .bg(rgba_alpha(fg, 0.05))
                            .child(file.path.clone()),
                    );
                    for hunk in &file.hunks {
                        // ハンクヘッダ
                        root = root.child(
                            div()
                                .px_3()
                                .py(px(1.0))
                                .text_size(px(9.0))
                                .text_color(hsla(theme.ansi[6])) // cyan
                                .child(hunk.header.clone()),
                        );
                        for line in &hunk.lines {
                            let (prefix, color, bg_color) = match line.kind {
                                tako_core::DiffLineKind::Add => (
                                    "+",
                                    theme.green, // 緑
                                    rgba_alpha(theme.green, 0.1),
                                ),
                                tako_core::DiffLineKind::Remove => (
                                    "-",
                                    theme.red, // 赤
                                    rgba_alpha(theme.red, 0.1),
                                ),
                                tako_core::DiffLineKind::Context => (
                                    " ",
                                    fg,
                                    Rgba {
                                        r: 0.0,
                                        g: 0.0,
                                        b: 0.0,
                                        a: 0.0,
                                    },
                                ),
                            };
                            root = root.child(
                                div()
                                    .px_3()
                                    .text_size(px(11.0))
                                    .text_color(hsla(color))
                                    .bg(bg_color)
                                    .child(format!("{prefix}{}", line.content)),
                            );
                        }
                    }
                }
            }
        }

        root
    }

    /// ファイルツリーの root をアクティブタブ内全ペインの cwd に追従させる（FR-3.1）。
    /// render・トグル・定期ループから呼ばれる。非表示中は何もしない。
    /// is_dir() の stat syscall は毎フレーム呼ぶと重いため省略し、
    /// 2 秒の refresh() で存在しないディレクトリを回収する
    fn sync_filetree_roots(&mut self) {
        if !self.filetree.visible {
            return;
        }
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        let active_tab_id = self.workspace.active_tab().id();

        // フォアグラウンドペイン
        for pane in self.workspace.active_tab().tree().panes() {
            let Some(cwd) = self.terminals.get(&pane.id()).and_then(|s| s.cwd()) else {
                continue;
            };
            let cwd = cwd.to_path_buf();
            if !roots.contains(&cwd) {
                roots.push(cwd);
            }
        }

        // バックグラウンドペイン（同タブ由来のみ）
        for bp in self.workspace.shelved_panes() {
            if bp.origin_tab() != active_tab_id {
                continue;
            }
            let Some(cwd) = self.terminals.get(&bp.id()).and_then(|s| s.cwd()) else {
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

    /// 全ペインから Claude TUI のメトリクス（ctx%/usage）を収集・更新する
    fn refresh_agent_metrics(&mut self) {
        let mut best: Option<AgentMetrics> = None;
        // フォーカスペインを優先し、なければ他の alt_screen ペインから取得
        let focused = self.workspace.active_tab().tree().focused();
        let pane_ids: Vec<PaneId> = std::iter::once(focused)
            .chain(
                self.workspace
                    .tabs()
                    .iter()
                    .flat_map(|tab| tab.tree().panes().into_iter().map(|p| p.id()))
                    .filter(|id| *id != focused),
            )
            .collect();
        for pid in pane_ids {
            if let Some(session) = self.terminals.get(&pid) {
                if let Some(m) = session.agent_metrics() {
                    if m.ctx_percent.is_some() || m.usage_text.is_some() {
                        best = Some(m);
                        break;
                    }
                }
            }
        }
        if let Some(m) = best {
            self.agent_metrics = m;
        } else {
            self.agent_metrics = AgentMetrics::default();
        }
    }

    /// 左サイドバーのファイルツリー（FR-3.1。非表示なら None = 純粋なターミナル FR-3.7）。
    /// 「タブ = ワークスペース」: タブ内全ペインの cwd がワークスペースフォルダとして並ぶ
    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.filetree.visible {
            return None;
        }
        let theme = self.theme.clone();
        let tab_title = self.workspace.active_tab().title().to_string();
        let sidebar_path = self.active_tab_cwd().map(|p| {
            if let Ok(home) = std::env::var("HOME") {
                if let Ok(rel) = p.strip_prefix(&home) {
                    return format!("~/{}", rel.display());
                }
            }
            p.display().to_string()
        });
        // プレビュー表示中のファイル（開いている行を控えめにハイライトする）
        let open_paths: std::collections::HashSet<std::path::PathBuf> =
            self.previews.values().map(|p| p.path.clone()).collect();
        let mut rows = self.filetree.rows();
        // 新規ファイル/フォルダ用の仮行を親の直後に挿入
        let inline_new_insert = match &self.inline_edit {
            Some(edit) if edit.kind != InlineEditKind::Rename => {
                let parent = &edit.parent;
                // 親ディレクトリの子の末尾（展開済み子孫をすべて飛ばした直後）に挿入
                let insert_pos =
                    rows.iter()
                        .position(|r| r.entry.path == *parent)
                        .map(|parent_idx| {
                            let parent_depth = rows[parent_idx].depth;
                            let mut end = parent_idx + 1;
                            while end < rows.len() && rows[end].depth > parent_depth {
                                end += 1;
                            }
                            end
                        });
                insert_pos.map(|pos| {
                    let depth = rows
                        .get(pos.saturating_sub(1))
                        .filter(|r| r.entry.path == *parent)
                        .map(|r| r.depth + 1)
                        .unwrap_or_else(|| {
                            rows.get(pos.saturating_sub(1))
                                .map(|r| r.depth)
                                .unwrap_or(1)
                        });
                    (pos, depth)
                })
            }
            _ => None,
        };
        if let Some((pos, depth)) = inline_new_insert {
            let edit = self.inline_edit.as_ref().unwrap();
            rows.insert(
                pos,
                filetree::Row {
                    entry: filetree::Entry {
                        path: edit.parent.join("__inline_new__"),
                        name: String::new(),
                        is_dir: edit.kind == InlineEditKind::NewDir,
                    },
                    depth,
                    expanded: false,
                    root: false,
                    git_status: None,
                },
            );
        }
        let inline_edit_snapshot = self.inline_edit.clone();
        Some(
            div()
                .w(px(SIDEBAR_WIDTH))
                .h_full()
                .flex()
                .flex_col()
                .bg(rgba(theme.mantle))
                .border_r_1()
                .border_color(hsla(theme.border_subtle))
                .text_size(px(12.0))
                .text_color(hsla(theme.foreground))
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(38.0))
                        .px(px(12.0))
                        .flex_none()
                        .child(
                            div()
                                .w(px(14.0))
                                .h(px(11.0))
                                .flex_none()
                                .relative()
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(0.0))
                                        .left(px(0.0))
                                        .w(px(6.0))
                                        .h(px(4.0))
                                        .rounded_t(px(1.5))
                                        .bg(hsla(theme.accent)),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top(px(3.0))
                                        .left(px(0.0))
                                        .w(px(14.0))
                                        .h(px(8.0))
                                        .rounded(px(1.5))
                                        .bg(hsla(theme.accent)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.text_secondary))
                                        .child(SharedString::from(truncate(&tab_title, 20))),
                                )
                                .children(sidebar_path.map(|path| {
                                    div()
                                        .text_size(px(10.5))
                                        .font_family("Monaco")
                                        .text_color(hsla(theme.tab_inactive_foreground))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(truncate(&path, 28)))
                                })),
                        ),
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
                            // インライン編集中の行を検出
                            let is_inline = match &inline_edit_snapshot {
                                Some(edit) if edit.kind == InlineEditKind::Rename => {
                                    path == edit.parent
                                }
                                Some(edit) if path == edit.parent.join("__inline_new__") => true,
                                _ => false,
                            };
                            if is_inline {
                                let edit = inline_edit_snapshot.as_ref().unwrap();
                                let depth = row.depth;
                                let indent = 8.0 + 12.0 * depth as f32;
                                let icon = match edit.kind {
                                    InlineEditKind::Rename => {
                                        if is_dir {
                                            "🗂 "
                                        } else {
                                            ""
                                        }
                                    }
                                    InlineEditKind::NewFile => "📄 ",
                                    InlineEditKind::NewDir => "🗂 ",
                                };
                                let before_cursor = &edit.text[..edit.cursor];
                                let after_cursor = &edit.text[edit.cursor..];
                                return div()
                                    .id(("filetree-row", index as u64))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .w_full()
                                    .px_1()
                                    .pl(px(indent))
                                    .bg(rgba_alpha(theme.tab_active_background, 0.8))
                                    .child(SharedString::from(icon.to_string()))
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_row()
                                            .border_1()
                                            .border_color(hsla(theme.accent))
                                            .rounded_sm()
                                            .px(px(2.0))
                                            .bg(rgba(theme.background))
                                            .child(SharedString::from(before_cursor.to_string()))
                                            .child(
                                                div()
                                                    .w(px(1.0))
                                                    .h(px(14.0))
                                                    .bg(hsla(theme.foreground))
                                                    .flex_none(),
                                            )
                                            .child(SharedString::from(after_cursor.to_string())),
                                    );
                            }
                            let is_open = !is_dir && open_paths.contains(&path);
                            let drag_path = path.clone();
                            let base = div()
                                .id(("filetree-row", index as u64))
                                .flex()
                                .flex_row()
                                .items_center()
                                .w_full()
                                .py(px(1.0))
                                .cursor_pointer()
                                .hover(|d| d.bg(rgba(theme.surface_hover)))
                                .on_click(cx.listener({
                                    let ctx_path = path.clone();
                                    move |this, _: &gpui::ClickEvent, _, cx| {
                                        if is_dir {
                                            this.filetree.toggle_dir(&ctx_path);
                                        } else {
                                            this.open_file_row(&ctx_path, cx);
                                        }
                                        cx.notify();
                                    }
                                }))
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener({
                                        let ctx_path = path.clone();
                                        move |this, e: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.context_menu = Some(ContextMenu {
                                                path: ctx_path.clone(),
                                                is_dir,
                                                position: e.position,
                                            });
                                            cx.notify();
                                        }
                                    }),
                                )
                                // ファイルは D&D でドロップ位置にプレビューとして開ける（FR-3.11）
                                .on_drag(
                                    FileDrag { path: drag_path },
                                    self.drag_ghost_builder(
                                        DragKind::File,
                                        format!(
                                            "{} {}",
                                            if is_dir { "🗂" } else { "📄" },
                                            truncate(&row.entry.name, 24)
                                        ),
                                        cx,
                                    ),
                                );
                            if row.root {
                                // ワークスペースフォルダの見出し行: 太字 + 上仕切り線（2 つ目以降）
                                base.when(index > 0, |d| {
                                    d.border_t_1()
                                        .border_color(hsla_alpha(theme.pane_border, 0.6))
                                        .mt_1()
                                })
                                .py(px(2.0))
                                .gap(px(4.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(hsla(theme.tab_active_foreground))
                                // chevron (SVG)
                                .child(
                                    svg()
                                        .path(file_icons::chevron_icon(row.expanded).svg_path())
                                        .size(px(14.0))
                                        .flex_none()
                                        .text_color(hsla(theme.tab_inactive_foreground)),
                                )
                                // folder icon (SVG)
                                .child(
                                    svg()
                                        .path(file_icons::folder_icon(row.expanded).svg_path())
                                        .size(px(16.0))
                                        .flex_none()
                                        .text_color(hsla(theme.accent)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(truncate(&row.entry.name, 22))),
                                )
                            } else {
                                let git_marker = row.git_status.map(|gs| match gs {
                                    filetree::GitChange::Modified => ("M", theme.yellow),
                                    filetree::GitChange::Added => ("A", theme.green),
                                    filetree::GitChange::Deleted => ("D", theme.red),
                                    filetree::GitChange::Renamed => ("R", theme.accent),
                                    filetree::GitChange::Untracked => {
                                        ("?", theme.tab_inactive_foreground)
                                    }
                                });
                                let indent = 12.0 + 12.0 * row.depth as f32;
                                let mut row_el = base
                                    .pl(px(indent))
                                    .py(px(2.0))
                                    .gap(px(4.0))
                                    .when(!is_dir, |d| d.text_color(hsla(theme.text_tertiary)))
                                    .when(is_open, |d| {
                                        d.bg(rgba_alpha(theme.accent, 0.13))
                                            .text_color(hsla(theme.foreground))
                                            .shadow(vec![BoxShadow {
                                                color: hsla(theme.accent),
                                                offset: point(px(2.), px(0.)),
                                                blur_radius: px(0.),
                                                spread_radius: px(0.),
                                                inset: true,
                                            }])
                                    });
                                if is_dir {
                                    let folder_color = if row.expanded {
                                        theme.accent
                                    } else {
                                        theme.tab_inactive_foreground
                                    };
                                    // chevron (SVG)
                                    row_el = row_el.child(
                                        svg()
                                            .path(file_icons::chevron_icon(row.expanded).svg_path())
                                            .size(px(14.0))
                                            .flex_none()
                                            .text_color(hsla(theme.tab_inactive_foreground)),
                                    );
                                    // folder icon (SVG)
                                    row_el = row_el.child(
                                        svg()
                                            .path(file_icons::folder_icon(row.expanded).svg_path())
                                            .size(px(16.0))
                                            .flex_none()
                                            .text_color(hsla(folder_color)),
                                    );
                                } else {
                                    // file: chevron 分のスペーサー + SVG file icon
                                    row_el = row_el.child(div().w(px(14.0)).flex_none());
                                    let icon_kind = file_icons::resolve_file_icon(
                                        std::path::Path::new(&row.entry.name),
                                    );
                                    let icon_color = match icon_kind.color_category() {
                                        file_icons::IconColor::Green => theme.green,
                                        file_icons::IconColor::Accent => theme.accent,
                                        file_icons::IconColor::Peach => theme.peach,
                                        file_icons::IconColor::Mauve => theme.mauve,
                                        file_icons::IconColor::Yellow => theme.yellow,
                                        file_icons::IconColor::Dim => theme.tab_inactive_foreground,
                                    };
                                    row_el = row_el.child(
                                        svg()
                                            .path(icon_kind.svg_path())
                                            .size(px(16.0))
                                            .flex_none()
                                            .text_color(hsla(icon_color)),
                                    );
                                }
                                // ファイル/フォルダ名
                                row_el = row_el.child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(SharedString::from(truncate(&row.entry.name, 24))),
                                );
                                // git status マーカー
                                row_el = row_el.children(git_marker.map(|(label, color)| {
                                    div()
                                        .text_size(px(10.5))
                                        .font_family("Monaco")
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(hsla(color))
                                        .flex_none()
                                        .pr(px(8.0))
                                        .child(SharedString::from(label.to_string()))
                                }));
                                row_el
                            }
                        })),
                ),
        )
    }

    /// コンテキストメニューの描画（FR-3.12）
    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let ctx = self.context_menu.as_ref()?;
        let theme = &self.theme;
        let path = ctx.path.clone();
        let is_dir = ctx.is_dir;
        let pos = ctx.position;
        let items: Vec<(&str, &str)> = vec![
            ("copy-rel", "相対パスをコピー"),
            ("copy-abs", "絶対パスをコピー"),
            ("reveal", "Finder で表示"),
            ("open-term", "ターミナルで開く"),
            ("sep1", ""),
            ("rename", "名前変更"),
            ("new-file", "新しいファイル"),
            ("new-dir", "新しいフォルダ"),
            ("sep2", ""),
            ("trash", "削除"),
        ];
        let menu = div()
            .absolute()
            .left(pos.x)
            .top(pos.y)
            .w(px(180.0))
            .py(px(4.0))
            .bg(rgba(theme.tab_bar_background))
            .border_1()
            .border_color(hsla(theme.pane_border))
            .rounded_md()
            .text_size(px(12.0))
            .text_color(hsla(theme.foreground))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .children(items.into_iter().enumerate().map(|(i, (id, label))| {
                if id.starts_with("sep") {
                    return div()
                        .h(px(1.0))
                        .mx_1()
                        .my(px(2.0))
                        .bg(hsla_alpha(theme.pane_border, 0.5))
                        .into_any_element();
                }
                let path = path.clone();
                div()
                    .id(("ctx-item", i as u64))
                    .w_full()
                    .px_2()
                    .py(px(2.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.tab_active_background)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.context_menu = None;
                        this.handle_context_action(id, &path, is_dir, cx);
                    }))
                    .when(id == "trash", |d| d.text_color(hsla(theme.red)))
                    .child(SharedString::from(label.to_string()))
                    .into_any_element()
            }));
        let backdrop = div()
            .id("ctx-backdrop")
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                }),
            )
            .child(menu);
        Some(backdrop.into_any_element())
    }

    fn handle_inline_edit_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        match ks.key.as_str() {
            "enter" => {
                self.commit_inline_edit(cx);
            }
            "escape" => {
                self.inline_edit = None;
                cx.notify();
            }
            "backspace" => {
                if let Some(ref mut edit) = self.inline_edit {
                    if edit.cursor > 0 {
                        let prev = edit.text[..edit.cursor]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        edit.text.drain(prev..edit.cursor);
                        edit.cursor = prev;
                    }
                }
                cx.notify();
            }
            "delete" => {
                if let Some(ref mut edit) = self.inline_edit {
                    if edit.cursor < edit.text.len() {
                        let next = edit.text[edit.cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| edit.cursor + i)
                            .unwrap_or(edit.text.len());
                        edit.text.drain(edit.cursor..next);
                    }
                }
                cx.notify();
            }
            "left" => {
                if let Some(ref mut edit) = self.inline_edit {
                    if edit.cursor > 0 {
                        edit.cursor = edit.text[..edit.cursor]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                    }
                }
                cx.notify();
            }
            "right" => {
                if let Some(ref mut edit) = self.inline_edit {
                    if edit.cursor < edit.text.len() {
                        edit.cursor = edit.text[edit.cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| edit.cursor + i)
                            .unwrap_or(edit.text.len());
                    }
                }
                cx.notify();
            }
            "home" => {
                if let Some(ref mut edit) = self.inline_edit {
                    edit.cursor = 0;
                }
                cx.notify();
            }
            "end" => {
                if let Some(ref mut edit) = self.inline_edit {
                    edit.cursor = edit.text.len();
                }
                cx.notify();
            }
            _ => {
                if let Some(ch) = ks.key_char.as_ref() {
                    if !ch.is_empty() && !ks.modifiers.control && !ks.modifiers.platform {
                        if let Some(ref mut edit) = self.inline_edit {
                            edit.text.insert_str(edit.cursor, ch);
                            edit.cursor += ch.len();
                        }
                        cx.notify();
                    }
                }
            }
        }
    }

    fn commit_inline_edit(&mut self, cx: &mut Context<Self>) {
        use tako_control::protocol::{FileOpKind, Request};
        let Some(edit) = self.inline_edit.take() else {
            return;
        };
        let name = edit.text.trim().to_string();
        if name.is_empty() {
            cx.notify();
            return;
        }
        let (op, path_str) = match edit.kind {
            InlineEditKind::Rename => (FileOpKind::Rename, edit.parent.display().to_string()),
            InlineEditKind::NewFile => (FileOpKind::CreateFile, edit.parent.display().to_string()),
            InlineEditKind::NewDir => (FileOpKind::CreateDir, edit.parent.display().to_string()),
        };
        let _ = tako_control::dispatch(
            self,
            Request::FileOp {
                op,
                path: path_str,
                name: Some(name),
                pane: None,
            },
            PaneOrigin::User,
        );
        self.sync_filetree_roots();
        cx.notify();
    }

    /// コンテキストメニューのアクション実行（FR-3.12）
    fn handle_context_action(
        &mut self,
        action: &str,
        path: &std::path::Path,
        _is_dir: bool,
        cx: &mut Context<Self>,
    ) {
        use tako_control::protocol::{FileOpKind, Request};
        let path_str = path.display().to_string();
        match action {
            "copy-abs" => {
                if let Ok(result) = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::CopyAbsolutePath,
                        path: path_str,
                        name: None,
                        pane: None,
                    },
                    PaneOrigin::User,
                ) {
                    if let Some(p) = result["path"].as_str() {
                        cx.write_to_clipboard(ClipboardItem::new_string(p.to_string()));
                    }
                }
            }
            "copy-rel" => {
                let pane = self.focused_pane().as_u64();
                if let Ok(result) = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::CopyRelativePath,
                        path: path_str,
                        name: None,
                        pane: Some(pane),
                    },
                    PaneOrigin::User,
                ) {
                    if let Some(p) = result["path"].as_str() {
                        cx.write_to_clipboard(ClipboardItem::new_string(p.to_string()));
                    }
                }
            }
            "reveal" => {
                let _ = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::Reveal,
                        path: path_str,
                        name: None,
                        pane: None,
                    },
                    PaneOrigin::User,
                );
            }
            "open-term" => {
                let pane = self.focused_pane().as_u64();
                let _ = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::OpenTerminal,
                        path: path_str,
                        name: None,
                        pane: Some(pane),
                    },
                    PaneOrigin::User,
                );
            }
            "rename" | "new-file" | "new-dir" => {
                let init_text = if action == "rename" {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    String::new()
                };
                let cursor = init_text.len();
                self.inline_edit = Some(InlineEdit {
                    parent: if action == "rename" || path.is_dir() {
                        path.to_path_buf()
                    } else {
                        path.parent().unwrap_or(path).to_path_buf()
                    },
                    kind: match action {
                        "rename" => InlineEditKind::Rename,
                        "new-file" => InlineEditKind::NewFile,
                        _ => InlineEditKind::NewDir,
                    },
                    text: init_text,
                    cursor,
                });
                if action != "rename" {
                    if let Some(parent_path) = if path.is_dir() {
                        Some(path.to_path_buf())
                    } else {
                        path.parent().map(|p| p.to_path_buf())
                    } {
                        self.filetree.expand_dir(&parent_path);
                    }
                }
            }
            "trash" => {
                let _ = tako_control::dispatch(
                    self,
                    Request::FileOp {
                        op: FileOpKind::Trash,
                        path: path_str,
                        name: None,
                        pane: None,
                    },
                    PaneOrigin::User,
                );
                self.sync_filetree_roots();
            }
            _ => {}
        }
        cx.notify();
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
                direction: None,
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ファイルを開けない: {e}");
        }
        self.drain_pending_highlights(cx);
        cx.notify();
    }

    /// プレビューの「コード ⇔ Markdown」トグル（目アイコン。FR-3.3）。
    /// 同じ状態は dispatch（OpenFile の mode 指定）= CLI / MCP からも切り替えられる。
    /// Image / Pdf モードではトグルしない
    fn toggle_preview_mode(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(state) = self.previews.get(&pane_id) else {
            return;
        };
        let mode = match state.mode {
            preview::PreviewMode::Code => preview::PreviewMode::Markdown,
            preview::PreviewMode::Markdown => preview::PreviewMode::Code,
            preview::PreviewMode::Image
            | preview::PreviewMode::Pdf
            | preview::PreviewMode::Video => return,
        };
        let path = state.path.clone();
        let (new_state, raw) = preview::load_fast(&path, mode);
        self.previews.insert(pane_id, new_state);
        if let Some(text) = raw {
            self.spawn_highlight(pane_id, path, text, cx);
        }
        cx.notify();
    }

    /// syntect ハイライトを background executor で実行し、完了後にプレビューを差し替える
    fn spawn_highlight(
        &self,
        pane: PaneId,
        path: std::path::PathBuf,
        text: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let task = cx
                .background_executor()
                .spawn(async move { preview::highlight_text(&p, &text) });
            let lines = task.await;
            let _ = this.update(cx, |app, cx| {
                if let Some(state) = app.previews.get_mut(&pane) {
                    if state.path == path {
                        state.content = preview::PreviewContent::Code(lines);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// UI から直接 dispatch した場合の pending_highlights を処理する
    fn drain_pending_highlights(&mut self, cx: &mut Context<Self>) {
        for (pane, path, text) in std::mem::take(&mut self.pending_highlights) {
            self.spawn_highlight(pane, path, text, cx);
        }
    }

    // --- D&D（FR-2.16.10 tmux セッション取り込み / FR-3.11 ファイルプレビュー） ---

    /// `on_drag` のコンストラクタを作る共通部: drag_kind を記録してゴーストチップを返す。
    /// gpui はドラッグ開始時にこのコンストラクタを 1 回呼ぶ（= ドラッグ開始フック）
    fn drag_ghost_builder<T: 'static>(
        &self,
        kind: DragKind,
        label: String,
        cx: &mut Context<Self>,
    ) -> impl Fn(&T, Point<Pixels>, &mut Window, &mut App) -> gpui::Entity<DragGhost> + 'static
    {
        let entity = cx.entity().downgrade();
        let theme = self.theme.clone();
        move |_, _, _, cx| {
            let _ = entity.update(cx, |this, cx| {
                this.drag_kind = Some(kind);
                this.drop_target = None;
                cx.notify();
            });
            cx.new(|_| DragGhost {
                label: label.clone(),
                theme: theme.clone(),
            })
        }
    }

    /// ドロップ先オーバーレイの on_drag_move: カーソルのペイン内位置から挿入位置を更新する。
    /// capture phase で全ペインのオーバーレイに届くため、矩形内判定は自前で行う
    fn update_drop_target(
        &mut self,
        pane_id: PaneId,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        kind: DragKind,
        cx: &mut Context<Self>,
    ) {
        let new = bounds.contains(&position).then(|| {
            let fx =
                f32::from(position.x - bounds.origin.x) / f32::from(bounds.size.width).max(1.0);
            let fy =
                f32::from(position.y - bounds.origin.y) / f32::from(bounds.size.height).max(1.0);
            (pane_id, drop_zone(fx, fy, kind == DragKind::File))
        });
        match new {
            Some(target) => {
                if self.drop_target != Some(target) {
                    self.drop_target = Some(target);
                    cx.notify();
                }
            }
            // このペインから出た（別ペインのオーバーレイが新しい位置を立てる）
            None => {
                if self.drop_target.is_some_and(|(p, _)| p == pane_id) {
                    self.drop_target = None;
                    cx.notify();
                }
            }
        }
    }

    /// ドロップ確定の共通処理: ドラッグ状態を畳み、対象ペインで確定した挿入位置を返す
    fn take_drop_zone(&mut self, pane_id: PaneId) -> Option<DropZone> {
        self.drag_kind = None;
        self.drop_target
            .take()
            .filter(|(p, _)| *p == pane_id)
            .map(|(_, zone)| zone)
    }

    /// tmux セッション行のドロップ（FR-2.16.10）: CLI / MCP と同じ dispatch `TmuxOpen` で
    /// ドロップ位置のペインを分割し attach 表示する。セッション起動（pending_attach）の
    /// 後処理は Split 系と同じ（項目 56 の罠）
    fn drop_tmux_session(
        &mut self,
        pane_id: PaneId,
        drag: TmuxSessionDrag,
        cx: &mut Context<Self>,
    ) {
        let zone = self.take_drop_zone(pane_id).unwrap_or(DropZone::Right);
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::TmuxOpen {
                socket: drag.socket,
                session: drag.name,
                window: drag.window,
                pane: Some(pane_id.as_u64()),
                direction: Some(zone_to_direction(zone)),
            },
            PaneOrigin::User,
        );
        match result {
            Ok(_) => {
                for (pane, options) in std::mem::take(&mut self.pending_attach) {
                    if let Err(e) = self.spawn_session(pane, options, cx) {
                        eprintln!("warning: 取り込みペインを開けない: {e}");
                        self.remove_pane(pane, cx);
                    }
                }
                for (pane, data) in std::mem::take(&mut self.pending_writes) {
                    if let Some(session) = self.terminals.get(&pane) {
                        session.write(data);
                    }
                }
            }
            Err(e) => eprintln!("warning: tmux セッションを取り込めない: {e}"),
        }
        cx.notify();
    }

    /// ペインタイトルバーのドロップ（FR-1.10）: dispatch `MovePane`（target + direction）で
    /// 同タブ内の挿し直し。自分自身へのドロップは no-op
    fn drop_pane(&mut self, pane_id: PaneId, drag: PaneDrag, cx: &mut Context<Self>) {
        let zone = self.take_drop_zone(pane_id).unwrap_or(DropZone::Right);
        if drag.pane == pane_id {
            cx.notify();
            return;
        }
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::MovePane {
                pane: Some(drag.pane.as_u64()),
                tab: None,
                target: Some(pane_id.as_u64()),
                direction: Some(zone_to_direction(zone)),
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ペインを移動できない: {e}");
        }
        cx.notify();
    }

    /// ファイル行のドロップ（FR-3.11 / FR-3.13）:
    /// ターミナルペイン中央 → パス文字列を send、それ以外 → ファイルを開く
    fn drop_file(&mut self, pane_id: PaneId, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let zone = self.take_drop_zone(pane_id).unwrap_or(DropZone::Center);
        let is_terminal =
            self.terminals.contains_key(&pane_id) && !self.previews.contains_key(&pane_id);
        if is_terminal && zone == DropZone::Center {
            let path_str = path.display().to_string();
            let escaped = if path_str
                .chars()
                .any(|c| c == ' ' || c == '\'' || c == '"' || c == '(' || c == ')')
            {
                format!("'{}'", path_str.replace('\'', "'\\\\''"))
            } else {
                path_str
            };
            let _ = tako_control::dispatch(
                self,
                tako_control::protocol::Request::Send {
                    pane: Some(pane_id.as_u64()),
                    text: escaped,
                    newline: false,
                    tmux_session: None,
                },
                PaneOrigin::User,
            );
            cx.notify();
            return;
        }
        let direction = match zone {
            DropZone::Center => None,
            zone => Some(zone_to_direction(zone)),
        };
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::OpenFile {
                pane: Some(pane_id.as_u64()),
                path: path.display().to_string(),
                mode: None,
                direction,
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ファイルを開けない: {e}");
        }
        self.drain_pending_highlights(cx);
        cx.notify();
    }

    /// ペイン 1 枚分のドロップ先オーバーレイ（D&D 中のみ生成）。
    /// ホバー中はドロップ後に新ペインが占める半面をハイライトし、結果ラベルを出す
    /// （FR-2.16.10 / FR-3.11 の「ドロップしたらこうなる」挿入プレビュー）
    fn render_drop_target(
        &self,
        pane_id: PaneId,
        rect: Rect,
        kind: DragKind,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = &self.theme;
        let zone = self
            .drop_target
            .filter(|(p, _)| *p == pane_id)
            .map(|(_, zone)| zone);
        let mut overlay = div()
            .id(("drop-target", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .on_drag_move::<TmuxSessionDrag>(cx.listener(
                move |this, e: &DragMoveEvent<TmuxSessionDrag>, _, cx| {
                    this.update_drop_target(
                        pane_id,
                        e.bounds,
                        e.event.position,
                        DragKind::TmuxSession,
                        cx,
                    );
                },
            ))
            .on_drag_move::<FileDrag>(cx.listener(
                move |this, e: &DragMoveEvent<FileDrag>, _, cx| {
                    this.update_drop_target(
                        pane_id,
                        e.bounds,
                        e.event.position,
                        DragKind::File,
                        cx,
                    );
                },
            ))
            .on_drag_move::<PaneDrag>(cx.listener(
                move |this, e: &DragMoveEvent<PaneDrag>, _, cx| {
                    // 自分自身の上では挿入プレビューを出さない（移動にならないため）
                    if e.drag(cx).pane == pane_id {
                        if this.drop_target.is_some_and(|(p, _)| p == pane_id) {
                            this.drop_target = None;
                            cx.notify();
                        }
                        return;
                    }
                    this.update_drop_target(
                        pane_id,
                        e.bounds,
                        e.event.position,
                        DragKind::Pane,
                        cx,
                    );
                },
            ))
            .on_drag_move::<BackgroundPaneDrag>(cx.listener(
                move |this, e: &DragMoveEvent<BackgroundPaneDrag>, _, cx| {
                    this.update_drop_target(
                        pane_id,
                        e.bounds,
                        e.event.position,
                        DragKind::BackgroundPane,
                        cx,
                    );
                },
            ))
            .on_drop::<TmuxSessionDrag>(cx.listener(move |this, drag: &TmuxSessionDrag, _, cx| {
                this.drop_tmux_session(pane_id, drag.clone(), cx);
            }))
            .on_drop::<FileDrag>(cx.listener(move |this, drag: &FileDrag, _, cx| {
                this.drop_file(pane_id, drag.path.clone(), cx);
            }))
            .on_drop::<PaneDrag>(cx.listener(move |this, drag: &PaneDrag, _, cx| {
                this.drop_pane(pane_id, *drag, cx);
            }))
            .on_drop::<BackgroundPaneDrag>(cx.listener(
                move |this, drag: &BackgroundPaneDrag, _, cx| {
                    this.drop_background_pane(pane_id, *drag, cx);
                },
            ));
        if let Some(zone) = zone {
            let (left, top, w, h) = match zone {
                DropZone::Left => (0.0, 0.0, 0.5, 1.0),
                DropZone::Right => (0.5, 0.0, 0.5, 1.0),
                DropZone::Up => (0.0, 0.0, 1.0, 0.5),
                DropZone::Down => (0.0, 0.5, 1.0, 0.5),
                DropZone::Center => (0.0, 0.0, 1.0, 1.0),
            };
            let label = match (kind, zone) {
                (DragKind::TmuxSession, _) => "ここに分割して表示",
                (DragKind::File, DropZone::Center) => "ここで開く",
                (DragKind::File, _) => "ここに分割して開く",
                (DragKind::Pane, _) => "この位置に移動",
                (DragKind::BackgroundPane, _) => "ここに復帰",
                (DragKind::Tab, _) => "この位置に移動",
            };
            overlay = overlay.child(
                div()
                    .absolute()
                    .left(relative(left))
                    .top(relative(top))
                    .w(relative(w))
                    .h(relative(h))
                    .rounded_sm()
                    .bg(rgba_alpha(theme.accent, 0.18))
                    .border_2()
                    .border_color(hsla(theme.accent))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(theme.tab_bar_background))
                            .text_size(px(11.0))
                            .text_color(hsla(theme.foreground))
                            .child(label),
                    ),
            );
        }
        overlay
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
                | self.preview_selecting.take().is_some()
                | self.dragging_pin.take().is_some()
                | std::mem::take(&mut self.dragging_panel)
            {
                cx.notify();
            }
            return;
        }
        // ピン留めウィンドウのタイトルバー D&D 移動（FR-2.16.15）
        if let Some((target, offset)) = self.dragging_pin {
            let vw = f32::from(window.viewport_size().width);
            let vh = f32::from(window.viewport_size().height);
            // タイトルバーが掴める範囲に左上をクランプ（画面外へ飛ばさない）
            let x = (f32::from(event.position.x) - f32::from(offset.x))
                .clamp(0.0, (vw - 40.0).max(0.0));
            let y = (f32::from(event.position.y) - f32::from(offset.y))
                .clamp(0.0, (vh - PIN_TITLE_BAR).max(0.0));
            if let Some(p) = self.pinned_previews.iter_mut().find(|p| p.target == target) {
                p.pos = point(px(x), px(y));
            }
            cx.notify();
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
        if let Some(pid) = self.preview_selecting {
            if let Some(pos) = self.preview_hit_test(pid, event.position) {
                if let Some(sel) = self.preview_selections.get_mut(&pid) {
                    sel.head = pos;
                    cx.notify();
                }
            }
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
        // D&D の後始末（ドロップ成立時は on_drop が stop_propagation 込みで先に畳む。
        // ここはドロップ先以外で離した場合のクリア）
        if self.drag_kind.take().is_some() | self.drop_target.take().is_some() {
            cx.notify();
            return;
        }
        if self.dragging_border.take().is_some()
            | self.dragging_scrollbar.take().is_some()
            | self.dragging_pin.take().is_some()
            | std::mem::take(&mut self.dragging_panel)
        {
            cx.notify();
            return;
        }
        self.preview_selecting = None;
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
        let Some(cell) = self.cell_size_for_pane(pane_id) else {
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

    /// 動画プレイヤーを起動し、再生を開始する
    fn start_video_player(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self.video_players.contains_key(&pane_id) {
            return;
        }
        let path = match self.previews.get(&pane_id) {
            Some(state) => state.path.clone(),
            None => return,
        };
        match video_player::VideoPlayer::open(&path) {
            Ok(mut player) => {
                player.play();
                self.video_players.insert(pane_id, player);
                self.ensure_video_ticker(cx);
                cx.notify();
            }
            Err(e) => {
                eprintln!("動画プレイヤー起動失敗: {e}");
            }
        }
    }

    /// 再生中の動画フレームを定期取得するティッカー（~30fps）。
    /// 再生中プレイヤーが無くなったら自動停止する
    fn ensure_video_ticker(&mut self, cx: &mut Context<Self>) {
        if self.video_ticker {
            return;
        }
        self.video_ticker = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                let live = this
                    .update(cx, |app, cx| {
                        let mut any_playing = false;
                        let mut any_new_frame = false;
                        for player in app.video_players.values_mut() {
                            if player.state == video_player::PlaybackState::Playing {
                                any_playing = true;
                                if player.grab_frame() {
                                    any_new_frame = true;
                                }
                            }
                        }
                        if any_new_frame {
                            cx.notify();
                        }
                        any_playing
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
            }
            this.update(cx, |app, _| app.video_ticker = false).ok();
        })
        .detach();
    }

    /// シークバー上のクリック位置から再生位置を計算してシークする。
    /// シークバー要素自体の bounds を使い、クリック x 座標→比率→秒数に変換
    fn video_seek_by_click(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        duration: f64,
        cx: &mut Context<Self>,
    ) {
        let bar_bounds = self.video_seek_bar_bounds.get(&pane_id).copied();
        if let (Some(bounds), Some(player)) = (bar_bounds, self.video_players.get_mut(&pane_id)) {
            let frac = ((f32::from(position.x) - f32::from(bounds.origin.x))
                / f32::from(bounds.size.width))
            .clamp(0.0, 1.0);
            let new_time = frac as f64 * duration;
            player.seek(new_time);
            player.grab_frame();
            cx.notify();
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

    const FONT_SIZE_MIN: f32 = 8.0;
    const FONT_SIZE_MAX: f32 = 32.0;
    const FONT_SIZE_STEP: f32 = 1.0;

    /// ペイン単位のフォントサイズ（オーバーライド未設定ならテーマ既定）
    fn pane_font_size(&self, pane_id: PaneId) -> f32 {
        self.pane_font_sizes
            .get(&pane_id)
            .copied()
            .unwrap_or(self.theme.font_size)
    }

    /// ペイン単位のline_height（font_size と同じ比率でスケール）
    fn pane_line_height(&self, pane_id: PaneId) -> f32 {
        let fs = self.pane_font_size(pane_id);
        self.theme.line_height * fs / self.theme.font_size
    }

    /// ペイン単位のセル寸法を計算（キャッシュあり）
    fn measure_pane_cell(&mut self, pane_id: PaneId, window: &mut Window) -> Size<Pixels> {
        if let Some(cell) = self.pane_cell_sizes.get(&pane_id) {
            return *cell;
        }
        let fs = self.pane_font_size(pane_id);
        let lh = self.pane_line_height(pane_id);
        let font = Font {
            family: SharedString::from(self.theme.font_family.clone()),
            ..gpui::font(self.theme.font_family.clone())
        };
        let font_id = window.text_system().resolve_font(&font);
        let width = window
            .text_system()
            .advance(font_id, px(fs), 'M')
            .map(|advance| advance.width)
            .unwrap_or(px(fs * 0.6));
        let cell = size(width, px(lh));
        self.pane_cell_sizes.insert(pane_id, cell);
        cell
    }

    /// ペイン単位の TextStyle を生成する
    fn pane_text_style(&self, pane_id: PaneId) -> TextStyle {
        let fs = self.pane_font_size(pane_id);
        let lh = self.pane_line_height(pane_id);
        TextStyle {
            color: hsla(self.theme.foreground),
            font_family: SharedString::from(self.theme.font_family.clone()),
            font_size: px(fs).into(),
            line_height: px(lh).into(),
            ..TextStyle::default()
        }
    }

    fn cell_size_for_pane(&self, pane_id: PaneId) -> Option<Size<Pixels>> {
        self.pane_cell_sizes
            .get(&pane_id)
            .copied()
            .or(self.cell_size)
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
                // タブ内ペイン状態の集約ドット（FR-2.1.4）
                let agg = CommandState::aggregate(
                    tab.tree()
                        .panes()
                        .iter()
                        .filter_map(|p| self.terminals.get(&p.id()))
                        .map(|s| s.command_state()),
                );
                let dot_color = match agg {
                    CommandState::Failed(_) => theme.red,
                    CommandState::Running => theme.accent,
                    CommandState::Idle => theme.green,
                    CommandState::Unknown => theme.text_overlay,
                };
                (id, label, dot_color)
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .w_full()
            .bg(rgba(theme.tab_bar_background))
            .border_b_1()
            .border_color(hsla(theme.border_subtle))
            .children(tabs.into_iter().map(|(id, label, dot_color)| {
                let is_active = id == active;
                let pane_count = self
                    .workspace
                    .tabs()
                    .iter()
                    .find(|t| t.id() == id)
                    .map(|t| t.tree().panes().len())
                    .unwrap_or(0);
                div()
                    .id(("tab", id.as_u64()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .h_full()
                    .px_3()
                    .cursor_pointer()
                    .when(is_active, |d| {
                        d.bg(rgba(theme.tab_active_background))
                            .shadow(vec![BoxShadow {
                                color: hsla(theme.accent),
                                offset: point(px(0.), px(-2.)),
                                blur_radius: px(0.),
                                spread_radius: px(0.),
                                inset: true,
                            }])
                    })
                    .when(!is_active, |d| d.hover(|d| d.bg(rgba(theme.surface_1))))
                    .text_color(if is_active {
                        hsla(theme.tab_active_foreground)
                    } else {
                        hsla(theme.tab_inactive_foreground)
                    })
                    .text_size(px(12.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let _ = this.workspace.activate_tab(id);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(7.0))
                            .h(px(7.0))
                            .rounded_full()
                            .bg(hsla(dot_color))
                            .shadow(vec![BoxShadow {
                                color: hsla_alpha(dot_color, 0.4),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(3.0),
                                spread_radius: px(0.),
                                inset: false,
                            }]),
                    )
                    .on_drag(
                        TabDrag { tab: id },
                        self.drag_ghost_builder(DragKind::Tab, truncate(&label, 24), cx),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(truncate(&label, 24))),
                    )
                    .when(pane_count > 1, |d| {
                        d.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(SharedString::from(format!("\u{00B7} {pane_count}"))),
                        )
                    })
                    .child(
                        div()
                            .id(("tab-bg", id.as_u64()))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .hover(|d| {
                                d.bg(rgba(theme.surface_highlight))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.background_tab(id, cx);
                            }))
                            .child("ー"),
                    )
                    .child(
                        div()
                            .id(("tab-close", id.as_u64()))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .hover(|d| {
                                d.bg(rgba(theme.surface_highlight))
                                    .text_color(hsla(theme.red))
                            })
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
                    .w(px(34.0))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(px(15.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .hover(|d| {
                        d.bg(rgba(theme.surface_highlight))
                            .text_color(hsla(theme.tab_active_foreground))
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.new_tab(cx)))
                    .child("+"),
            )
            .child(div().flex_grow(1.0))
            // 右端クローム: 設定アイコン（スペック準拠 30x30px border-radius 7px）
            .child(
                div()
                    .id("tab-settings")
                    .w(px(30.0))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(px(14.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .hover(|d| {
                        d.bg(rgba(theme.surface_highlight))
                            .text_color(hsla(theme.tab_active_foreground))
                    })
                    .child("⚙"),
            )
    }

    /// バックグラウンドからのドロップ（FR-2.15.3）
    fn drop_background_pane(
        &mut self,
        target_pane: PaneId,
        drag: BackgroundPaneDrag,
        cx: &mut Context<Self>,
    ) {
        let zone = self.drop_target.take().map(|(_, z)| z);
        let direction = match zone {
            Some(DropZone::Left) => SplitDirection::Left,
            Some(DropZone::Right) | None => SplitDirection::Right,
            Some(DropZone::Up) => SplitDirection::Up,
            Some(DropZone::Down) => SplitDirection::Down,
            Some(DropZone::Center) => SplitDirection::Right,
        };
        if let Err(e) = self
            .workspace
            .unshelve_pane(drag.pane, target_pane, direction)
        {
            eprintln!("warning: バックグラウンドから復帰できない: {e}");
        }
        self.drag_kind = None;
        if self.workspace.shelved_panes().is_empty() {
            self.drawer_visible = false;
        }
        cx.notify();
    }

    /// バックグラウンドドロワーのカード 1 枚（実画面サムネイル + タイトルバー）。タブ別グループ内で
    /// 1 ペインずつ描く（FR-2.15.6）。復帰は由来タブへ戻す（無ければアクティブタブ）
    fn render_shelf_card(
        &self,
        entry: &BackgroundEntry,
        pending_kill: Option<PaneId>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let pane_id = entry.pane;
        let label = entry.label.clone();
        let state_color = match entry.state {
            CommandState::Failed(_) => Some(theme.red),
            CommandState::Running => Some(theme.accent),
            CommandState::Idle => Some(theme.yellow),
            _ => None,
        };
        let is_pending_kill = pending_kill == Some(pane_id);
        // 実画面プレビュー（カーソルは出さずサムネイルらしくする）
        let lines = self.terminal_screen_lines(pane_id, false);

        // カードタイトルバー（通常ペインと同じスタイル。D&D でペインエリアへ復帰可能）
        let mut titlebar = div()
            .id(("shelf-titlebar", pane_id.as_u64()))
            .h(px(PANE_TITLE_BAR))
            .flex_none()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .bg(rgba(theme.tab_bar_background))
            .text_size(px(11.0))
            .text_color(hsla(theme.tab_inactive_foreground))
            .cursor(CursorStyle::OpenHand)
            .on_drag(
                BackgroundPaneDrag { pane: pane_id },
                self.drag_ghost_builder(DragKind::BackgroundPane, truncate(&label, 24), cx),
            );

        if is_pending_kill {
            // kill 確認（FR-2.15.2）。tmux セッションごと完全破棄する
            titlebar = titlebar
                .child(
                    div()
                        .flex_1()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .text_color(hsla(theme.red))
                        .child("完全に破棄?"),
                )
                .child(
                    div()
                        .id(("shelf-kill-yes", pane_id.as_u64()))
                        .cursor_pointer()
                        .text_color(hsla(theme.red))
                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.2)))
                        .px_1()
                        .rounded_sm()
                        .child("はい")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bg_pending_kill = None;
                            if this.workspace.remove_shelved(pane_id).is_some() {
                                this.terminals.remove(&pane_id);
                                this.previews.remove(&pane_id);
                                this.scroll_accum.remove(&pane_id);
                                this.scroll_ctls.remove(&pane_id);
                                this.drop_tmux_view_session(pane_id);
                                this.drop_backend_session(pane_id);
                            }
                            if this.workspace.shelved_panes().is_empty() {
                                this.drawer_visible = false;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("shelf-kill-no", pane_id.as_u64()))
                        .cursor_pointer()
                        .text_color(hsla(theme.tab_inactive_foreground))
                        .hover(|d| d.bg(rgba_alpha(theme.tab_active_background, 0.5)))
                        .px_1()
                        .rounded_sm()
                        .child("いいえ")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bg_pending_kill = None;
                            cx.notify();
                        })),
                );
        } else {
            if let Some(color) = state_color {
                titlebar =
                    titlebar.child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(hsla(color)));
            }
            titlebar = titlebar
                .child(
                    div()
                        .flex_1()
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .text_color(hsla(theme.foreground))
                        .child(SharedString::from(truncate(&label, 40))),
                )
                // 復帰ボタン（× の隣。FR-2.15.3。由来タブへ戻す）
                .child(
                    div()
                        .id(("shelf-restore", pane_id.as_u64()))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(hsla(theme.accent))
                        .hover(|d| d.bg(rgba_alpha(theme.accent, 0.2)))
                        .child("復帰")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let origin = this.workspace.shelved_origin_tab(pane_id);
                            let target = origin
                                .and_then(|t| this.workspace.get_tab(t))
                                .map(|t| t.tree().focused())
                                .unwrap_or_else(|| this.workspace.active_tab().tree().focused());
                            if let Err(e) =
                                this.workspace
                                    .unshelve_pane(pane_id, target, SplitDirection::Right)
                            {
                                eprintln!("warning: バックグラウンドから復帰できない: {e}");
                            }
                            if this.workspace.shelved_panes().is_empty() {
                                this.drawer_visible = false;
                            }
                            cx.notify();
                        })),
                )
                // kill ボタン（右上の ×。完全破棄の確認を開始。FR-2.15.2）
                .child(
                    div()
                        .id(("shelf-kill", pane_id.as_u64()))
                        .w(px(16.0))
                        .h(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                        .hover(|d| {
                            d.bg(rgba_alpha(theme.red, 0.25))
                                .text_color(hsla(theme.foreground))
                        })
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.bg_pending_kill = Some(pane_id);
                            cx.notify();
                        })),
                );
        }

        // カード本文 = ターミナルの実画面プレビュー（通常ペインと同じ行描画）。
        // ターミナルを持たないバックグラウンド（プレビューペイン等）はラベルだけ示す
        let body = if lines.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                .child(SharedString::from(truncate(&label, 24)))
        } else {
            div()
                .flex_1()
                .p(px(PANE_PADDING))
                .overflow_hidden()
                .bg(rgba(theme.background))
                .children(lines)
        };

        div()
            .id(("shelf-card", pane_id.as_u64()))
            .flex_none()
            .w(px(BG_CARD_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(if is_pending_kill {
                hsla(theme.red)
            } else {
                hsla(theme.pane_border)
            })
            .bg(rgba(theme.background))
            .child(titlebar)
            .child(body)
    }

    /// バックグラウンドドロワーの描画（FR-2.15。下部、ステータスバーの上に展開）。
    /// バックグラウンドターミナルは由来タブごとにグループ分けして横並び表示する（FR-2.15.6）
    fn render_drawer(&mut self, cx: &mut Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        if !self.drawer_visible {
            return None;
        }
        let theme = self.theme.clone();
        // 由来タブごとにグループ化（FR-2.15.6 バックグラウンドエリアのタブ別分離）。生存タブはタブ順、
        // 由来タブが閉じたものは「タブ <名前>（閉じたタブ）」として後ろにまとめる
        let mut bg_groups: Vec<(String, Vec<BackgroundEntry>)> = Vec::new();
        for tab in self.workspace.tabs() {
            let entries = self.background_entries_of_tab(tab.id());
            if !entries.is_empty() {
                bg_groups.push((tab.title().to_string(), entries));
            }
        }
        for closed in self.tmux_view_closed_origin_background() {
            bg_groups.push((format!("{}（閉じたタブ）", closed.title), closed.entries));
        }
        let bg_total: usize = bg_groups.iter().map(|(_, e)| e.len()).sum();

        let pending_kill = self.bg_pending_kill;

        // バックグラウンドターミナルをカード本文の寸法へリサイズし、通常ペインと同じ見え方の
        // プレビュー（実画面サムネイル）にする。resize は冪等なので毎フレーム呼んでも安全。
        // グループ見出しの高さ分も本文から差し引く
        let body_h = (self.drawer_height
            - DRAWER_HEADER_HEIGHT
            - DRAWER_GROUP_HEADER
            - PANE_TITLE_BAR
            - PANE_PADDING * 2.0
            - PANE_BORDER * 2.0
            - 8.0)
            .max(40.0);
        if let Some(cell) = self.cell_size {
            let cols = ((BG_CARD_WIDTH - PANE_BORDER * 2.0 - PANE_PADDING * 2.0)
                / f32::from(cell.width))
            .floor() as usize;
            let rows = (body_h / f32::from(cell.height)).floor() as usize;
            let cw = f32::from(cell.width).round() as u16;
            let ch = f32::from(cell.height).round() as u16;
            let ids: Vec<PaneId> = bg_groups
                .iter()
                .flat_map(|(_, e)| e.iter().map(|x| x.pane))
                .collect();
            for pane_id in ids {
                if let Some(session) = self.terminals.get_mut(&pane_id) {
                    session.resize(cols, rows, cw, ch);
                }
            }
        }

        // タブ別グループを横並びに配置し、横スクロールで全件閲覧できるようにする（FR-2.15.6）
        let mut cards = div()
            .id("drawer-cards")
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.0))
            .gap_2()
            .px_2()
            .py_1()
            .overflow_x_scroll();

        if bg_groups.is_empty() {
            cards = cards.child(
                div()
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .py_1()
                    .child("バックグラウンドのターミナルはありません"),
            );
        } else {
            for (gi, (title, entries)) in bg_groups.iter().enumerate() {
                let mut group = div()
                    .id(("drawer-group", gi as u64))
                    .flex()
                    .flex_col()
                    .h_full()
                    .gap_1()
                    // グループ間の区切り（最初以外の左に縦線）でタブ別の塊を視認させる
                    .when(gi > 0, |d| {
                        d.pl_2()
                            .border_l_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.6))
                    })
                    .child(
                        // 親タブ見出し（FR-2.15.6。バックグラウンドペインの親タブを明記）
                        div()
                            .h(px(DRAWER_GROUP_HEADER))
                            .flex_none()
                            .flex()
                            .items_center()
                            .text_size(px(10.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(format!(
                                "タブ {}（{}）",
                                truncate(title, 18),
                                entries.len()
                            ))),
                    );
                let mut row = div().flex().flex_row().flex_1().min_h(px(0.0)).gap_2();
                for entry in entries {
                    row = row.child(self.render_shelf_card(entry, pending_kill, cx));
                }
                group = group.child(row);
                cards = cards.child(group);
            }
        }

        Some(
            div()
                .id("drawer-drop-target")
                .flex()
                .flex_col()
                .flex_none()
                .h(px(self.drawer_height))
                .w_full()
                .bg(rgba(theme.crust))
                .border_t_1()
                .border_color(hsla(theme.border_subtle))
                .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| {
                    this.background_tab(drag.tab, cx);
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .flex_none()
                        .h(px(DRAWER_HEADER_HEIGHT))
                        .px_2()
                        .text_size(px(10.0))
                        .text_color(hsla(theme.tab_inactive_foreground))
                        .child(SharedString::from(format!(
                            "バックグラウンドのターミナル（{}）",
                            bg_total
                        )))
                        .child(div().flex_grow(1.0))
                        .child(
                            div()
                                .id("drawer-close")
                                .cursor_pointer()
                                .hover(|d| d.text_color(hsla(theme.foreground)))
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.drawer_visible = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(cards),
        )
    }

    /// 下部ステータスバー（FR-2.16.4。Zed / VSCode 風）。
    /// 左 = トグル群（Files / BG）、中央 = フリート表示、右 = 使用量 / コンテキスト / tmux / git
    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme.clone();
        let agents_dot =
            match CommandState::aggregate(self.terminals.values().map(|s| s.command_state())) {
                CommandState::Failed(_) => Some(theme.red),
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
                .text_size(px(10.5))
                .when(active, |d| {
                    d.text_color(hsla(theme.accent))
                        .bg(rgba_alpha(theme.accent, 0.1))
                })
                .when(!active, |d| d.text_color(hsla(theme.text_tertiary)))
                .hover(|d| d.bg(rgba(theme.surface_hover)))
                .border_r_1()
                .border_color(hsla(theme.border_subtle))
        };
        // オーケストレーター情報の収集
        let fleet_label = {
            let has_master = self
                .workspace
                .tabs()
                .iter()
                .flat_map(|tab| tab.tree().panes())
                .any(|p| {
                    p.role().is_some_and(|r| {
                        r == "orchestrator-master" || r.starts_with("orchestrator-master:")
                    })
                });
            if has_master {
                let worker_count: usize = self
                    .workspace
                    .tabs()
                    .iter()
                    .flat_map(|tab| tab.tree().panes())
                    .filter(|p| {
                        p.role()
                            .is_some_and(|r| r.starts_with("orchestrator-worker:"))
                    })
                    .count();
                Some(worker_count)
            } else {
                None
            }
        };
        // コンテキストメーター（Claude TUI フッターからの実データ）
        let ctx_pct = self.agent_metrics.ctx_percent.unwrap_or(0);
        let ctx_bar_color = if ctx_pct >= 90 {
            theme.red
        } else if ctx_pct >= 70 {
            theme.yellow
        } else {
            theme.accent
        };
        let ctx_fill_frac = ctx_pct as f32 / 100.0;
        let ctx_detail = self.agent_metrics.ctx_detail.clone();
        let usage_text = self.agent_metrics.usage_text.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .w_full()
            .bg(rgba(theme.tab_bar_background))
            .border_t_1()
            .border_color(hsla(theme.border_subtle))
            // Files トグル
            .child(
                toggle("statusbar-filetree", self.filetree.visible)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_filetree();
                        cx.notify();
                    }))
                    .child("Files"),
            )
            // BG トグル
            .child({
                let bg_count = self.workspace.shelved_panes().len();
                let drawer_open = self.drawer_visible;
                toggle("statusbar-bg", drawer_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.drawer_visible = !this.drawer_visible;
                        cx.notify();
                    }))
                    .on_drop::<TabDrag>(cx.listener(|this, drag: &TabDrag, _, cx| {
                        this.background_tab(drag.tab, cx);
                    }))
                    .child(if bg_count > 0 {
                        format!("BG {bg_count}")
                    } else {
                        "BG".into()
                    })
            })
            // フリート表示（オーケストレーター検出時のみ）
            .children(fleet_label.map(|worker_count| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .h_full()
                    .px_2()
                    .border_r_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.accent))
                            .child("⚙"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(theme.tab_active_foreground))
                            .child("master"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(format!(
                                "\u{00B7} {worker_count} workers"
                            ))),
                    )
            }))
            .child(div().flex_grow(1.0))
            // usage 表示（Claude TUI フッターからの検出値）
            .children(usage_text.map(|text| {
                // トークン数とコストを分離（"12.3K tok · $0.45" 形式を想定）
                let (tokens, cost) = if let Some(pos) = text.find('$') {
                    let tok_part = text[..pos].trim().trim_end_matches('·').trim();
                    let cost_part = text[pos..].trim();
                    (tok_part.to_string(), Some(cost_part.to_string()))
                } else {
                    (text.clone(), None)
                };
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .h_full()
                    .px_2()
                    .border_r_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.teal))
                            .child("📊"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child("usage"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.tab_active_foreground))
                            .child(SharedString::from(tokens)),
                    );
                if let Some(c) = cost {
                    row = row.child(
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.teal))
                            .child(SharedString::from(c)),
                    );
                }
                row
            }))
            // コンテキストメーター
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .h_full()
                    .px_2()
                    .border_r_1()
                    .border_color(hsla(theme.border_subtle))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child("ctx"),
                    )
                    .child(
                        div()
                            .w(px(70.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(rgba(theme.surface_highlight))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .rounded(px(3.0))
                                    .w(relative(ctx_fill_frac))
                                    .bg(hsla(ctx_bar_color)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.tab_active_foreground))
                            .child(SharedString::from(format!("{ctx_pct}%"))),
                    )
                    .children(ctx_detail.map(|detail| {
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(detail))
                    })),
            )
            // tmux トグル
            .child(
                toggle(
                    "statusbar-tmux",
                    self.panel_visible && self.panel_view == PanelView::Tmux,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_panel_view(PanelView::Tmux, cx);
                }))
                .children(agents_dot.map(|color| {
                    div()
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(hsla(color))
                        .shadow(vec![BoxShadow {
                            color: hsla_alpha(color, 0.6),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(4.0),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                }))
                .child("tmux"),
            )
            // git トグル
            .child(
                toggle(
                    "statusbar-git",
                    self.panel_visible && self.panel_view == PanelView::Git,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_panel_view(PanelView::Git, cx);
                }))
                .child({
                    let branch = self
                        .git_data
                        .as_ref()
                        .and_then(|d| d.branches.iter().find(|b| b.is_current))
                        .map(|b| truncate(&b.name, 16))
                        .unwrap_or_else(|| "git".into());
                    SharedString::from(format!("⎇ {branch}"))
                }),
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
            if view == PanelView::Git {
                self.refresh_git(cx);
            }
        }
        cx.notify();
    }

    /// git パネルのデータを即座に background 取得する
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        if let Some(cwd) = self.active_tab_cwd() {
            let selected = self.git_selected_commit.clone();
            cx.spawn(async move |this, cx| {
                let data = cx
                    .background_executor()
                    .spawn(async move { fetch_git_data(&cwd, selected.as_deref()) })
                    .await;
                let _ = this.update(cx, |app: &mut TakoApp, cx| {
                    app.git_data = data;
                    cx.notify();
                });
            })
            .detach();
        }
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
                .when(!active, |d| d.hover(|d| d.bg(rgba(theme.surface_hover))))
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
                .bg(rgba(theme.mantle))
                .border_l_1()
                .border_color(hsla(theme.border_subtle))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(2.0))
                        .px_2()
                        .h(px(38.0))
                        .flex_none()
                        .bg(rgba(theme.mantle))
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
                    PanelView::Git => self.render_git_view(cx).into_any_element(),
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

    /// プレビュー対象のラベル（ポップアップ / ピンのタイトルに使う。FR-2.16.13）
    fn preview_label(&self, target: PreviewTarget) -> String {
        match target {
            PreviewTarget::Pane(pane_id) => self.pane_preview_label(pane_id),
            PreviewTarget::ClosedGroup(tab) => {
                let title = self
                    .workspace
                    .shelved_panes()
                    .iter()
                    .find(|p| p.origin_tab() == tab)
                    .map(|p| p.origin_tab_title().to_string())
                    .unwrap_or_default();
                let count = self.background_entries_of_tab(tab).len();
                format!("タブ {}（閉じたタブ・{count} 件）", truncate(&title, 20))
            }
            PreviewTarget::TmuxWindow(pane_id, win) => {
                let win_name = self
                    .backend_windows
                    .get(&pane_id)
                    .and_then(|ws| ws.iter().find(|w| w.index == win))
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| format!("{win}"));
                let pane_label = self.pane_preview_label(pane_id);
                format!("{pane_label} · window {win}:{win_name}")
            }
        }
    }

    /// プレビュー対象が中身（サムネイルにできる端末）を持つか。空ならポップアップ /
    /// ピンを出さない（端末なしの単一ペイン・空グループ）
    fn preview_has_content(&self, target: PreviewTarget) -> bool {
        match target {
            PreviewTarget::Pane(pane_id) => self.terminals.contains_key(&pane_id),
            PreviewTarget::ClosedGroup(tab) => self
                .background_entries_of_tab(tab)
                .iter()
                .any(|e| self.terminals.contains_key(&e.pane)),
            PreviewTarget::TmuxWindow(pane_id, win) => {
                self.window_captures.contains_key(&(pane_id, win))
            }
        }
    }

    /// ペインの表示名（title / role > プレビュー名 > OSC タイトル > 既定）。
    /// tmux ビューの行ラベル（`tmux_view_groups`）と同じ優先順位で揃える。
    /// ツリー内・バックグラウンド中のどちらのペインも解決できる
    fn pane_preview_label(&self, pane_id: PaneId) -> String {
        let pane = self
            .workspace
            .tabs()
            .iter()
            .find_map(|t| t.tree().get(pane_id))
            .or_else(|| self.workspace.shelved(pane_id).map(|s| s.pane()));
        if let Some(p) = pane {
            match (p.title(), p.role()) {
                (Some(t), Some(r)) => return format!("{t} · {r}"),
                (Some(t), None) => return t.to_string(),
                (None, Some(r)) => return r.to_string(),
                (None, None) => {}
            }
        }
        if let Some(preview) = self.previews.get(&pane_id) {
            return format!("📄 {}", preview.file_name());
        }
        self.terminals
            .get(&pane_id)
            .and_then(|s| s.title())
            .unwrap_or("ターミナル")
            .to_string()
    }

    /// プレビュー本文（実画面サムネイル）。Pane は端末の現在グリッドをそのまま読む
    /// （リサイズしない＝バックグラウンドのプログラムを乱さない）。ClosedGroup はグループ内の
    /// 全バックグラウンドペインを均等高で縦に積む（FR-2.16.16）。ライブ更新は `on_term_event` が出力ごとに
    /// 呼ぶ `cx.notify()` の再描画で自動的に得られる
    fn preview_content(&self, target: PreviewTarget) -> gpui::Div {
        let theme = &self.theme;
        match target {
            PreviewTarget::Pane(pane_id) => div()
                .flex_1()
                .p(px(PANE_PADDING))
                .overflow_hidden()
                .bg(rgba(theme.background))
                .children(self.terminal_screen_lines(pane_id, false)),
            PreviewTarget::ClosedGroup(tab) => {
                let mut body = div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p(px(PANE_PADDING))
                    .overflow_hidden()
                    .bg(rgba(theme.background));
                for entry in self.background_entries_of_tab(tab) {
                    let lines = self.terminal_screen_lines(entry.pane, false);
                    body = body.child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .rounded_sm()
                            .border_1()
                            .border_color(hsla_alpha(theme.pane_border, 0.6))
                            .child(
                                div()
                                    .flex_none()
                                    .px_1()
                                    .bg(rgba(theme.tab_bar_background))
                                    .text_size(px(9.0))
                                    .text_color(hsla(theme.tab_inactive_foreground))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(truncate(&entry.label, 32))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .overflow_hidden()
                                    .children(lines),
                            ),
                    );
                }
                body
            }
            PreviewTarget::TmuxWindow(pane_id, win) => {
                let text_style = self.text_style();
                let lines = self
                    .window_captures
                    .get(&(pane_id, win))
                    .cloned()
                    .unwrap_or_default();
                let mut body = div()
                    .flex_1()
                    .p(px(PANE_PADDING))
                    .overflow_hidden()
                    .bg(rgba(theme.background));
                for line in lines {
                    body = body.child(
                        div().whitespace_nowrap().child(
                            StyledText::new(SharedString::from(line))
                                .with_default_highlights(&text_style, Vec::new()),
                        ),
                    );
                }
                body
            }
        }
    }

    /// プレビューの本文ボックス（タイトルバー + 実画面サムネイル）を組む。
    /// ホバーポップアップとピン留めウィンドウで共用する（FR-2.16.13）
    fn preview_body(
        &self,
        target: PreviewTarget,
        live: bool,
        extra_title: Option<gpui::Div>,
    ) -> gpui::Div {
        let theme = &self.theme;
        let label = self.preview_label(target);
        let mut titlebar = div()
            .h(px(PIN_TITLE_BAR))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .bg(rgba(theme.tab_bar_background))
            .text_size(px(11.0))
            .text_color(hsla(theme.tab_inactive_foreground))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(truncate(&label, 40))),
            );
        if live {
            titlebar = titlebar.child(
                div()
                    .flex_none()
                    .text_size(px(9.0))
                    .text_color(hsla(theme.accent))
                    .child("● LIVE"),
            );
        }
        if let Some(extra) = extra_title {
            titlebar = titlebar.child(extra);
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(titlebar)
            .child(self.preview_content(target))
    }

    /// ホバープレビューのポップアップ（FR-2.16.13 / FR-2.16.16）。マウス位置の左側に実画面
    /// サムネイルを出す（tmux ビューは右パネルにあるため左へ伸ばす）。読み取り専用（ピン留めは
    /// 行 / カード側のボタン）。ライブ更新は通常の再描画で得られる
    fn render_hover_preview(&self, window: &Window) -> Option<gpui::AnyElement> {
        let hp = self.hover_preview?;
        let theme = &self.theme;
        // 中身を持たない対象（プレビューペイン等でサムネイル無し）はポップアップを出さない
        if !self.preview_has_content(hp.target) {
            return None;
        }
        let viewport = window.viewport_size();
        let left = (f32::from(hp.anchor.x) - PREVIEW_POPUP_W - 12.0).max(8.0);
        let top = f32::from(hp.anchor.y)
            .min((f32::from(viewport.height) - PREVIEW_POPUP_H - 8.0).max(8.0))
            .max(8.0);
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(PREVIEW_POPUP_W))
                .h(px(PREVIEW_POPUP_H))
                .rounded_md()
                .overflow_hidden()
                .border_1()
                .border_color(hsla(theme.accent))
                .bg(rgba(theme.background))
                .child(self.preview_body(hp.target, true, None))
                .into_any_element(),
        )
    }

    /// ピン留めされた常駐プレビュー群（FR-2.16.15）。アプリ内フローティングウィンドウとして
    /// 絶対配置で描き、タイトルバー D&D で移動・× で解除。中身（端末グリッド）はライブ更新される。
    /// 対象が消えた（kill 等）ピンはこのフレームでは描かず、次の操作で掃除される
    fn render_pinned_previews(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let theme = self.theme.clone();
        // 借用衝突を避けるため対象リストを先に取り出す（PinnedPreview は Copy）
        let pins: Vec<PinnedPreview> = self.pinned_previews.clone();
        pins.into_iter()
            .filter(|pin| self.preview_has_content(pin.target))
            .map(|pin| {
                let target = pin.target;
                let key = pin_key(target);
                let label = self.preview_label(target);
                div()
                    .id(("pin", key))
                    .absolute()
                    .left(pin.pos.x)
                    .top(pin.pos.y)
                    .w(px(PIN_W))
                    .h(px(PIN_H))
                    .flex()
                    .flex_col()
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(hsla(theme.accent))
                    .bg(rgba(theme.background))
                    // ピン上の操作が下のペインへ抜けないようにする
                    .occlude()
                    .child(
                        // タイトルバー = ドラッグ移動ハンドル + ラベル + LIVE + × 解除
                        div()
                            .id(("pin-title", key))
                            .h(px(PIN_TITLE_BAR))
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .bg(rgba(theme.tab_bar_background))
                            .text_size(px(10.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .cursor(CursorStyle::OpenHand)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                    if let Some(p) =
                                        this.pinned_previews.iter().find(|p| p.target == target)
                                    {
                                        this.dragging_pin = Some((
                                            target,
                                            point(e.position.x - p.pos.x, e.position.y - p.pos.y),
                                        ));
                                    }
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(truncate(&label, 28))),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(9.0))
                                    .text_color(hsla(theme.accent))
                                    .child("● LIVE"),
                            )
                            .child(
                                div()
                                    .id(("pin-close", key))
                                    .flex_none()
                                    .px_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                                    .hover(|d| {
                                        d.bg(rgba_alpha(theme.red, 0.25))
                                            .text_color(hsla(theme.foreground))
                                    })
                                    .child("×")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_pin(target, Some(false));
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(self.preview_content(target))
                    .into_any_element()
            })
            .collect()
    }

    /// ターミナルの現在画面を行 div のリストへ変換する（通常ペイン描画とバックグラウンドプレビューで共用）。
    /// run ごとの色・太字・下線などの装飾を StyledText のハイライトへ写す。
    /// 全角文字を含む行はランごとにセル幅固定 div で配置し、フォント advance と
    /// グリッドセル幅のずれを吸収する（Markdown テーブル等の罫線崩れ防止）
    fn terminal_screen_lines(&self, pane_id: PaneId, show_cursor: bool) -> Vec<gpui::Div> {
        let theme = &self.theme;
        let has_custom_font = self.pane_font_sizes.contains_key(&pane_id);
        let default_style = if has_custom_font {
            self.pane_text_style(pane_id)
        } else {
            self.text_style()
        };
        let line_h = if has_custom_font {
            self.pane_line_height(pane_id)
        } else {
            theme.line_height
        };
        let cell_width = self
            .pane_cell_sizes
            .get(&pane_id)
            .map(|c| c.width)
            .or_else(|| self.cell_size.map(|c| c.width));
        let Some(screen) = self
            .terminals
            .get(&pane_id)
            .map(|s| s.screen_opts(theme, show_cursor))
        else {
            return Vec::new();
        };
        let _total_cols = screen.cols;
        screen
            .lines
            .into_iter()
            .map(|line| {
                if cell_width.is_none() {
                    // セル幅未計測: フォールバック（起動直後の一瞬のみ）
                    let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = line
                        .runs
                        .iter()
                        .map(|run| (run.range.clone(), self.run_highlight(run)))
                        .collect();
                    return div().h(px(line_h)).child(
                        StyledText::new(line.text)
                            .with_default_highlights(&default_style, highlights),
                    );
                }
                // 1 文字 = 1 div でグリッド位置に確実にスナップ。
                // 半角のグループ化は GPUI のテキストレイアウトとの不整合で
                // 文字消失を引き起こすため廃止（全角・半角とも個別 div）
                let cw = cell_width.unwrap();
                let chars: Vec<(usize, char)> = line.text.char_indices().collect();
                let row = div().h(px(line_h)).flex().flex_row().overflow_hidden();
                let mut children: Vec<gpui::AnyElement> = Vec::with_capacity(chars.len());
                for (ci, &(byte_off, ch)) in chars.iter().enumerate() {
                    let run = line
                        .runs
                        .iter()
                        .find(|r| byte_off >= r.range.start && byte_off < r.range.end)
                        .unwrap_or(line.runs.last().unwrap());
                    let char_cols = if ci + 1 < line.cell_cols.len() {
                        line.cell_cols[ci + 1] - line.cell_cols[ci]
                    } else {
                        1
                    };
                    let s = ch.to_string();
                    let hl = self.run_highlight(run);
                    let styled = StyledText::new(SharedString::from(s.clone()))
                        .with_default_highlights(&default_style, vec![(0..s.len(), hl)]);
                    let mut d = div()
                        .w(cw * char_cols.max(1) as f32)
                        .flex_none()
                        .overflow_hidden();
                    if let Some(bg) = run.bg {
                        d = d.bg(hsla(bg));
                    }
                    children.push(d.child(styled).into_any_element());
                }
                row.children(children)
            })
            .collect()
    }

    fn run_highlight(&self, run: &tako_core::screen::StyleRun) -> HighlightStyle {
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
            strikethrough: run.strikeout.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            }),
            fade_out: None,
        }
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
        let cell = self
            .pane_cell_sizes
            .get(&pane_id)
            .copied()
            .or(self.cell_size)
            .expect("render 冒頭で実測済み");

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

        // タイトルとロールを分離取得（ロールは独立バッジとして表示）
        let pane_info = self.workspace.active_tab().tree().get(pane_id);
        let pane_title = pane_info.and_then(|p| p.title().map(str::to_string));
        let pane_role = pane_info.and_then(|p| p.role().map(str::to_string));
        let title_label = pane_title
            .or_else(|| pane_role.clone())
            .or_else(|| {
                self.terminals
                    .get(&pane_id)
                    .and_then(|s| s.title())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "ターミナル".to_string());
        let role_badge = pane_role.as_deref().map(|r| {
            if r.contains("orchestrator-master") || r == "master" {
                ("ORCH", theme.accent, 0.14)
            } else if r.contains("orchestrator-worker") || r.starts_with("worker") {
                ("WORKER", theme.teal, 0.12)
            } else {
                (r.split(':').next().unwrap_or(r), theme.text_tertiary, 0.14)
            }
        });
        let (state_dot, state_label) = self
            .terminals
            .get(&pane_id)
            .map(|s| match s.command_state() {
                tako_core::CommandState::Failed(_) => (Some(theme.red), Some("failed")),
                tako_core::CommandState::Running => (Some(theme.accent), Some("running")),
                tako_core::CommandState::Idle => (Some(theme.green), Some("idle")),
                tako_core::CommandState::Unknown => (None, None),
            })
            .unwrap_or((None, None));

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
        let lines = self.terminal_screen_lines(pane_id, !scrolled_in_tmux);

        div()
            .id(("pane", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .bg(rgba(theme.background))
            .border(px(PANE_BORDER))
            .rounded(px(7.0))
            .border_color(if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.border_default)
            })
            .when(focused, |d| {
                d.shadow(vec![
                    BoxShadow {
                        color: hsla_alpha(theme.accent, 0.25),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                        inset: false,
                    },
                    BoxShadow {
                        color: gpui::hsla(0., 0., 0., 0.35),
                        offset: point(px(0.), px(8.)),
                        blur_radius: px(24.),
                        spread_radius: px(0.),
                        inset: false,
                    },
                ])
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
                div()
                    .id(("pane-titlebar", pane_id.as_u64()))
                    .h(px(PANE_TITLE_BAR))
                    .flex_none()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .bg(rgba(if focused {
                        theme.surface_2
                    } else {
                        theme.surface_0
                    }))
                    .border_b_1()
                    .border_color(hsla(if focused {
                        theme.border_default
                    } else {
                        theme.border_subtle
                    }))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .cursor(CursorStyle::OpenHand)
                    .on_drag(
                        PaneDrag { pane: pane_id },
                        self.drag_ghost_builder(DragKind::Pane, truncate(&title_label, 24), cx),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
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
                    // 状態ドット（スペック準拠: 6px + glow）
                    .children(state_dot.map(|color| {
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(hsla(color))
                            .shadow(vec![BoxShadow {
                                color: hsla_alpha(color, 0.5),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(4.0),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                    }))
                    // ペイン名
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_family("Monaco")
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if focused {
                                hsla(theme.foreground)
                            } else {
                                hsla(theme.tab_inactive_foreground)
                            })
                            .child(SharedString::from(truncate(&title_label, 40))),
                    )
                    // ロールバッジ
                    .children(role_badge.map(|(label, color, alpha)| {
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .text_color(hsla(color))
                            .bg(rgba_alpha(color, alpha))
                            .child(SharedString::from(label.to_string()))
                    }))
                    // 状態ラベル
                    .children(state_label.map(|label| {
                        div()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(label.to_string()))
                    }))
                    .child(div().flex_grow(1.0))
                    // ターミナル情報（シェル名 · cols×rows）
                    .child({
                        let shell_name = self
                            .terminals
                            .get(&pane_id)
                            .and_then(|s| s.title())
                            .unwrap_or("zsh");
                        let shell_short = shell_name.rsplit('/').next().unwrap_or(shell_name);
                        div()
                            .text_size(px(10.5))
                            .font_family("Monaco")
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .child(SharedString::from(format!(
                                "{shell_short} \u{00B7} {cols}\u{00D7}{rows}"
                            )))
                    })
                    // split ボタン
                    .child(
                        div()
                            .id(("pane-split", pane_id.as_u64()))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .hover(|d| {
                                d.bg(rgba(theme.surface_highlight))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.split_pane_button(pane_id, SplitDirection::Right, cx);
                            }))
                            .child("◫"),
                    )
                    // バックグラウンドボタン
                    .child(
                        div()
                            .id(("pane-bg", pane_id.as_u64()))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .hover(|d| {
                                d.bg(rgba(theme.surface_highlight))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.background_pane_button(pane_id, cx);
                            }))
                            .child("ー"),
                    )
                    // 閉じるボタン
                    .child(
                        div()
                            .id(("pane-close", pane_id.as_u64()))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(hsla(theme.tab_inactive_foreground))
                            .hover(|d| {
                                d.bg(rgba_alpha(theme.red, 0.25))
                                    .text_color(hsla(theme.red))
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

        let pdf_info: Option<usize> = if let preview::PreviewContent::Pdf(data) = &state.content {
            Some(data.total_pages)
        } else {
            None
        };

        // 選択状態
        let selection = self.preview_selections.get(&pane_id).cloned();

        // テキスト行を収集（選択テキスト抽出 + bounds 追跡用）
        let mut line_texts: Vec<String> = Vec::new();

        // 本文要素を先に組む（state の借用をここで終える）
        let body: Vec<gpui::AnyElement> = match &state.content {
            preview::PreviewContent::Code(lines) => {
                let number_width = lines.len().to_string().len();
                lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| {
                        let text: String = line.iter().map(|s| s.text.as_str()).collect();
                        let sel_range = selection
                            .as_ref()
                            .and_then(|s| s.range_for_line(i, text.len()));
                        line_texts.push(text);
                        self.preview_code_line_sel(
                            line,
                            Some((i + 1, number_width)),
                            sel_range,
                            pane_id,
                            i,
                            cx,
                        )
                        .into_any_element()
                    })
                    .collect()
            }
            preview::PreviewContent::Markdown(blocks) => blocks
                .iter()
                .enumerate()
                .map(|(i, block)| {
                    let text = md_block_plain_text(block);
                    let sel_range = selection
                        .as_ref()
                        .and_then(|s| s.range_for_line(i, text.len()));
                    line_texts.push(text);
                    self.preview_md_block_sel(block, sel_range, pane_id, i, cx)
                })
                .collect(),
            preview::PreviewContent::Image(data) => {
                let gpui_format = match data.format {
                    preview::ImageFileFormat::Png => gpui::ImageFormat::Png,
                    preview::ImageFileFormat::Jpeg => gpui::ImageFormat::Jpeg,
                    preview::ImageFileFormat::Gif => gpui::ImageFormat::Gif,
                    preview::ImageFileFormat::WebP => gpui::ImageFormat::Webp,
                    preview::ImageFileFormat::Svg => gpui::ImageFormat::Svg,
                };
                let image =
                    std::sync::Arc::new(gpui::Image::from_bytes(gpui_format, data.bytes.clone()));
                vec![div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui::img(image)
                            .object_fit(gpui::ObjectFit::Contain)
                            .max_w_full()
                            .max_h_full(),
                    )
                    .into_any_element()]
            }
            preview::PreviewContent::Pdf(data) => data
                .pages
                .iter()
                .enumerate()
                .filter(|(_, png)| !png.is_empty())
                .map(|(i, png)| {
                    let image = std::sync::Arc::new(gpui::Image::from_bytes(
                        gpui::ImageFormat::Png,
                        png.clone(),
                    ));
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .w_full()
                        .pb_2()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                                .pb_1()
                                .child(SharedString::from(format!(
                                    "— {} / {} —",
                                    i + 1,
                                    data.total_pages
                                ))),
                        )
                        .child(
                            gpui::img(image)
                                .object_fit(gpui::ObjectFit::Contain)
                                .max_w_full(),
                        )
                        .into_any_element()
                })
                .collect(),
            preview::PreviewContent::Video(data) => {
                let has_player = self.video_players.contains_key(&pane_id);
                let mut elements: Vec<gpui::AnyElement> = Vec::new();

                if has_player {
                    // AVFoundation プレイヤー起動中: キャッシュ済みフレームを表示
                    let player = self.video_players.get(&pane_id).unwrap();
                    let gen = player.frame_gen;
                    let need_update = match self.video_frame_cache.get(&pane_id) {
                        Some((cached_gen, _)) => *cached_gen != gen,
                        None => true,
                    };
                    if need_update && !player.current_bgra.is_empty() {
                        let w = player.width;
                        let h = player.height;
                        if let Some(rgba_img) =
                            image::RgbaImage::from_raw(w, h, player.current_bgra.clone())
                        {
                            let frame = image::Frame::new(rgba_img);
                            let render = std::sync::Arc::new(gpui::RenderImage::new(vec![frame]));
                            self.video_frame_cache.insert(pane_id, (gen, render));
                        }
                    }
                    if let Some((_, ref frame_image)) = self.video_frame_cache.get(&pane_id) {
                        let frame_image = frame_image.clone();
                        elements.push(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    gpui::img(frame_image)
                                        .object_fit(gpui::ObjectFit::Contain)
                                        .max_w_full()
                                        .flex_1(),
                                )
                                .into_any_element(),
                        );
                    }
                    let is_playing = player.state == video_player::PlaybackState::Playing;
                    let current_time = player.current_time;
                    let duration = player.duration;

                    // コントロールバー: 再生/一時停止 + シークバー + 時間表示
                    let play_btn_label: SharedString = if is_playing {
                        "\u{23f8}".into() // ⏸
                    } else {
                        "\u{25b6}\u{fe0e}".into() // ▶︎
                    };
                    let cur_m = current_time as u64 / 60;
                    let cur_s = current_time as u64 % 60;
                    let dur_m = duration as u64 / 60;
                    let dur_s = duration as u64 % 60;
                    let time_label: SharedString =
                        format!("{cur_m}:{cur_s:02} / {dur_m}:{dur_s:02}").into();
                    let progress_frac = if duration > 0.0 {
                        (current_time / duration).clamp(0.0, 1.0) as f32
                    } else {
                        0.0
                    };
                    let seek_dur = duration;
                    elements.push(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .bg(hsla_alpha(theme.background, 0.9))
                            .child(
                                div()
                                    .id(("video-toggle", pane_id.as_u64()))
                                    .cursor_pointer()
                                    .text_size(px(18.0))
                                    .child(play_btn_label)
                                    .on_click(cx.listener(
                                        move |this, _ev: &gpui::ClickEvent, _, cx| {
                                            if let Some(p) = this.video_players.get_mut(&pane_id) {
                                                p.toggle();
                                                this.ensure_video_ticker(cx);
                                                cx.notify();
                                            }
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id(("video-seek", pane_id.as_u64()))
                                    .relative()
                                    .flex_1()
                                    .h(px(6.0))
                                    .rounded(px(3.0))
                                    .bg(hsla_alpha(theme.foreground, 0.2))
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(3.0))
                                            .bg(hsla(theme.ansi[4]))
                                            .w(relative(progress_frac)),
                                    )
                                    .child({
                                        let entity = cx.entity().downgrade();
                                        canvas(
                                            |_, _, _| (),
                                            move |bounds, _, _, cx| {
                                                if let Some(e) = entity.upgrade() {
                                                    e.update(cx, |app, _| {
                                                        app.video_seek_bar_bounds
                                                            .insert(pane_id, bounds);
                                                    });
                                                }
                                            },
                                        )
                                        .absolute()
                                        .size_full()
                                    })
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(
                                            move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                                this.video_seek_by_click(
                                                    pane_id,
                                                    ev.position,
                                                    seek_dur,
                                                    cx,
                                                );
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(hsla_alpha(theme.foreground, 0.7))
                                    .child(time_label),
                            )
                            .into_any_element(),
                    );
                } else {
                    // プレイヤー未起動: ffmpeg サムネイル + 再生ボタン + メタ情報
                    if !data.thumbnail.is_empty() {
                        let image = std::sync::Arc::new(gpui::Image::from_bytes(
                            gpui::ImageFormat::Png,
                            data.thumbnail.clone(),
                        ));
                        elements.push(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .relative()
                                .p_2()
                                .child(
                                    gpui::img(image)
                                        .object_fit(gpui::ObjectFit::Contain)
                                        .max_w_full()
                                        .max_h(px(400.0)),
                                )
                                .into_any_element(),
                        );
                    }
                    // 再生ボタン
                    elements.push(
                        div()
                            .flex()
                            .justify_center()
                            .p_2()
                            .child(
                                div()
                                    .id(("video-play", pane_id.as_u64()))
                                    .cursor_pointer()
                                    .px_4()
                                    .py_1()
                                    .rounded(px(6.0))
                                    .bg(hsla(theme.ansi[4]))
                                    .text_color(hsla(theme.background))
                                    .text_size(px(14.0))
                                    .child(SharedString::from("\u{25b6}\u{fe0e} 再生"))
                                    .on_click(cx.listener(
                                        move |this, _ev: &gpui::ClickEvent, _, cx| {
                                            this.start_video_player(pane_id, cx);
                                        },
                                    )),
                            )
                            .into_any_element(),
                    );
                    // メタ情報
                    let mut info_lines = Vec::new();
                    if let Some((w, h)) = data.resolution {
                        info_lines.push(format!("解像度: {w} x {h}"));
                    }
                    if let Some(dur) = data.duration {
                        let mins = dur as u64 / 60;
                        let secs = dur as u64 % 60;
                        info_lines.push(format!("長さ: {mins}:{secs:02}"));
                    }
                    if let Some(codec) = &data.codec {
                        info_lines.push(format!("コーデック: {codec}"));
                    }
                    let size_mb = data.file_size as f64 / 1_000_000.0;
                    if size_mb >= 1.0 {
                        info_lines.push(format!("サイズ: {size_mb:.1} MB"));
                    } else {
                        info_lines
                            .push(format!("サイズ: {:.0} KB", data.file_size as f64 / 1_000.0));
                    }
                    elements.push(
                        div()
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(13.0))
                            .text_color(hsla_alpha(theme.foreground, 0.8))
                            .children(info_lines.into_iter().map(|line| {
                                div().child(SharedString::from(line)).into_any_element()
                            }))
                            .into_any_element(),
                    );
                }
                elements
            }
            preview::PreviewContent::Error(message) => vec![div()
                .p_2()
                .text_color(hsla(theme.red))
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
            .rounded(px(7.0))
            .border_color(if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.border_default)
            })
            .when(focused, |d| {
                d.shadow(vec![
                    BoxShadow {
                        color: hsla_alpha(theme.accent, 0.25),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                        inset: false,
                    },
                    BoxShadow {
                        color: gpui::hsla(0., 0., 0., 0.35),
                        offset: point(px(0.), px(8.)),
                        blur_radius: px(24.),
                        spread_radius: px(0.),
                        inset: false,
                    },
                ])
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
                    .bg(rgba(if focused {
                        theme.surface_2
                    } else {
                        theme.surface_0
                    }))
                    .border_b_1()
                    .border_color(hsla(if focused {
                        theme.border_default
                    } else {
                        theme.border_subtle
                    }))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .cursor(CursorStyle::OpenHand)
                    .on_drag(
                        PaneDrag { pane: pane_id },
                        self.drag_ghost_builder(
                            DragKind::Pane,
                            format!("📄 {}", truncate(&file_name, 24)),
                            cx,
                        ),
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
                                d.bg(rgba_alpha(theme.red, 0.25))
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
                            .child(SharedString::from({
                                let icon = match mode {
                                    preview::PreviewMode::Image => "🖼",
                                    preview::PreviewMode::Pdf => "📕",
                                    _ => "📄",
                                };
                                format!("{icon} {}", truncate(&file_name, 36))
                            })),
                    )
                    .child(div().flex_grow(1.0))
                    .children(md_capable.then(|| {
                        // 目アイコンのトグル（FR-3.3）: コード表示 ⇔ md レンダリング
                        let (icon, label) = match mode {
                            preview::PreviewMode::Markdown => ("</>", "コードとして表示"),
                            preview::PreviewMode::Code => ("👁", "md レンダリング表示"),
                            _ => ("", ""),
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
                    .children(pdf_info.map(|total| {
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                            .child(SharedString::from(format!("{} ページ", total)))
                    }))
                    .child(
                        div()
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.6))
                            .text_size(px(10.0))
                            .child(SharedString::from(truncate(&path_label, 40))),
                    ),
            )
            .child({
                // テキスト行を保存（選択テキスト抽出用）
                self.preview_line_texts.insert(pane_id, line_texts);
                // bounds 追跡用にリセット（各行の canvas で上書きされる）
                self.preview_line_bounds.insert(pane_id, Vec::new());

                div()
                    .id(("preview-scroll", pane_id.as_u64()))
                    .flex_1()
                    .p(px(PANE_PADDING + 4.0))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            if let Some(pos) = this.preview_hit_test(pane_id, ev.position) {
                                this.preview_selections.insert(
                                    pane_id,
                                    PreviewSelection {
                                        anchor: pos,
                                        head: pos,
                                    },
                                );
                                this.preview_selecting = Some(pane_id);
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseUpEvent, _, _cx| {
                            if this.preview_selecting == Some(pane_id) {
                                this.preview_selecting = None;
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if this.preview_selecting == Some(pane_id)
                            && ev.pressed_button == Some(MouseButton::Left)
                        {
                            if let Some(pos) = this.preview_hit_test(pane_id, ev.position) {
                                if let Some(sel) = this.preview_selections.get_mut(&pane_id) {
                                    sel.head = pos;
                                    cx.notify();
                                }
                            }
                        }
                    }))
                    .children(body)
                    .children(truncated.then(|| {
                        div()
                            .pt_2()
                            .text_size(px(11.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                            .child("…（大きいファイルのため末尾を省略して表示）")
                    }))
            })
    }

    /// ハイライト済みコード 1 行（行番号は固定幅左列、本文は残り幅で折り返す）
    fn preview_code_line(&self, line: &preview::Line, number: Option<(usize, usize)>) -> gpui::Div {
        let theme = &self.theme;
        let mut text = String::new();
        let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
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
            text.push(' ');
        }
        let code_el = StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
        if let Some((n, width)) = number {
            let num_label = format!("{n:>width$}  ");
            let num_len = num_label.len();
            div()
                .flex()
                .flex_row()
                .child(
                    div()
                        .flex_none()
                        .child(StyledText::new(num_label).with_default_highlights(
                            &self.text_style(),
                            vec![(
                                0..num_len,
                                HighlightStyle {
                                    color: Some(hsla_alpha(theme.tab_inactive_foreground, 0.5)),
                                    ..HighlightStyle::default()
                                },
                            )],
                        )),
                )
                .child(div().flex_1().min_w(px(0.0)).child(code_el))
        } else {
            div().child(code_el)
        }
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
                        Some(hsla(theme.yellow))
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

    /// Markdown ブロック 1 つの描画（FR-3.3。選択なし版は preview_md_block_sel に統合済み）
    #[allow(dead_code)]
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
                        div().flex_1().min_w(px(0.0)).child(
                            StyledText::new(text)
                                .with_default_highlights(&self.text_style(), highlights),
                        ),
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

    /// 選択ハイライト付きコード行 + bounds 追跡 canvas
    fn preview_code_line_sel(
        &self,
        line: &preview::Line,
        number: Option<(usize, usize)>,
        sel_range: Option<(usize, usize)>,
        pane_id: PaneId,
        line_idx: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = &self.theme;
        let mut text = String::new();
        let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
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
            text.push(' ');
        }
        // 選択ハイライト
        if let Some((start, end)) = sel_range {
            let s = snap_to_char_boundary(&text, start.min(text.len()));
            let e = snap_to_char_boundary(&text, end.min(text.len()));
            if s < e {
                highlights.push((
                    s..e,
                    HighlightStyle {
                        background_color: Some(hsla_alpha(theme.accent, 0.35)),
                        ..HighlightStyle::default()
                    },
                ));
            }
        }
        let code_el = StyledText::new(text).with_default_highlights(&self.text_style(), highlights);
        let entity = cx.entity().downgrade();
        let bounds_canvas = canvas(
            |_, _, _| (),
            move |bounds, _, _, cx| {
                if let Some(e) = entity.upgrade() {
                    e.update(cx, |app, _| {
                        let list = app.preview_line_bounds.entry(pane_id).or_default();
                        if list.len() <= line_idx {
                            list.resize(line_idx + 1, Bounds::default());
                        }
                        list[line_idx] = bounds;
                    });
                }
            },
        )
        .absolute()
        .size_full();

        if let Some((n, width)) = number {
            let num_label = format!("{n:>width$}  ");
            let num_len = num_label.len();
            div()
                .flex()
                .flex_row()
                .child(
                    div()
                        .flex_none()
                        .child(StyledText::new(num_label).with_default_highlights(
                            &self.text_style(),
                            vec![(
                                0..num_len,
                                HighlightStyle {
                                    color: Some(hsla_alpha(theme.tab_inactive_foreground, 0.5)),
                                    ..HighlightStyle::default()
                                },
                            )],
                        )),
                )
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(code_el)
                        .child(bounds_canvas),
                )
        } else {
            div().relative().child(code_el).child(bounds_canvas)
        }
    }

    /// 選択ハイライト付き Markdown ブロック + bounds 追跡 canvas
    fn preview_md_block_sel(
        &self,
        block: &preview::MdBlock,
        sel_range: Option<(usize, usize)>,
        pane_id: PaneId,
        line_idx: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let entity = cx.entity().downgrade();
        let bounds_canvas = canvas(
            |_, _, _| (),
            move |bounds, _, _, cx| {
                if let Some(e) = entity.upgrade() {
                    e.update(cx, |app, _| {
                        let list = app.preview_line_bounds.entry(pane_id).or_default();
                        if list.len() <= line_idx {
                            list.resize(line_idx + 1, Bounds::default());
                        }
                        list[line_idx] = bounds;
                    });
                }
            },
        )
        .absolute()
        .size_full();

        let add_sel = |highlights: &mut Vec<(std::ops::Range<usize>, HighlightStyle)>,
                       text: &str| {
            if let Some((start, end)) = sel_range {
                let s = snap_to_char_boundary(text, start.min(text.len()));
                let e = snap_to_char_boundary(text, end.min(text.len()));
                if s < e {
                    highlights.push((
                        s..e,
                        HighlightStyle {
                            background_color: Some(hsla_alpha(theme.accent, 0.35)),
                            ..HighlightStyle::default()
                        },
                    ));
                }
            }
        };

        match block {
            preview::MdBlock::Heading { level, spans } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_sel(&mut highlights, &text);
                let size = match level {
                    1 => 19.0,
                    2 => 16.0,
                    3 => 14.0,
                    _ => 13.0,
                };
                div()
                    .relative()
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
                    .child(bounds_canvas)
                    .into_any_element()
            }
            preview::MdBlock::Paragraph { spans } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_sel(&mut highlights, &text);
                div()
                    .relative()
                    .py_1()
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .child(bounds_canvas)
                    .into_any_element()
            }
            preview::MdBlock::ListItem {
                depth,
                marker,
                spans,
            } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_sel(&mut highlights, &text);
                div()
                    .relative()
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
                        div().flex_1().min_w(px(0.0)).child(
                            StyledText::new(text)
                                .with_default_highlights(&self.text_style(), highlights),
                        ),
                    )
                    .child(bounds_canvas)
                    .into_any_element()
            }
            preview::MdBlock::CodeBlock { lines } => div()
                .relative()
                .my_1()
                .p_2()
                .rounded_md()
                .bg(rgba_alpha(theme.tab_bar_background, 0.9))
                .flex()
                .flex_col()
                .children(lines.iter().map(|line| self.preview_code_line(line, None)))
                .child(bounds_canvas)
                .into_any_element(),
            preview::MdBlock::Quote { spans } => {
                let (text, mut highlights) = self.preview_md_text(spans);
                add_sel(&mut highlights, &text);
                div()
                    .relative()
                    .my_1()
                    .pl_2()
                    .border_l_2()
                    .border_color(hsla_alpha(theme.accent, 0.6))
                    .text_color(hsla_alpha(theme.foreground, 0.75))
                    .child(
                        StyledText::new(text)
                            .with_default_highlights(&self.text_style(), highlights),
                    )
                    .child(bounds_canvas)
                    .into_any_element()
            }
            preview::MdBlock::Rule => div()
                .relative()
                .my_2()
                .h(px(1.0))
                .bg(hsla_alpha(theme.pane_border, 0.9))
                .child(bounds_canvas)
                .into_any_element(),
        }
    }
}

/// Markdown ブロックのプレーンテキストを抽出する
fn snap_to_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn md_block_plain_text(block: &preview::MdBlock) -> String {
    match block {
        preview::MdBlock::Heading { spans, .. }
        | preview::MdBlock::Paragraph { spans }
        | preview::MdBlock::Quote { spans } => spans.iter().map(|s| s.text.as_str()).collect(),
        preview::MdBlock::ListItem { spans, .. } => spans.iter().map(|s| s.text.as_str()).collect(),
        preview::MdBlock::CodeBlock { lines } => lines
            .iter()
            .map(|line| line.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join(
                "
",
            ),
        preview::MdBlock::Rule => String::new(),
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

    fn queue_write(&mut self, pane: PaneId, data: Vec<u8>) {
        self.pending_writes.push((pane, data));
    }

    fn queue_write_on_alt_screen(&mut self, pane: PaneId, data: Vec<u8>) {
        self.alt_screen_writes
            .push((pane, data, std::time::Instant::now()));
    }

    fn queue_prompt_flow(&mut self, pane: PaneId, prompt: String) {
        let now = std::time::Instant::now();
        self.prompt_flows.push(PromptFlow {
            pane,
            prompt,
            state: PromptFlowState::WaitAltScreen,
            created_at: now,
            state_entered_at: now,
        });
    }

    fn detach_session(&mut self, pane: PaneId) {
        self.terminals.remove(&pane);
        self.previews.remove(&pane);
        self.scroll_accum.remove(&pane);
        self.scroll_ctls.remove(&pane);
        self.drop_tmux_view_session(pane);
        self.drop_backend_session(pane);
    }

    fn track_tmux_view(
        &mut self,
        pane: PaneId,
        session: String,
        wrapper: Option<String>,
        socket: Option<String>,
    ) {
        self.tmux_view_panes.insert(
            pane,
            TmuxViewTarget {
                session,
                wrapper,
                socket,
            },
        );
    }

    /// orphan tmux セッションの一括クリーンアップ（FR-2.16.11）。現存ペイン・バックグラウンドペインの
    /// backend セッション、表示中ビューの元/ラッパー名を protected として渡し、backend
    /// socket 上の取り残しだけを kill する。tmux 永続化 OFF / tmux 不在では何もしない
    fn cleanup_orphan_tmux(&self) -> Vec<String> {
        if !self.tmux_persist || !tako_core::tmux_backend::available() {
            return Vec::new();
        }
        let mut protected: std::collections::HashSet<String> =
            self.backend_sessions.values().cloned().collect();
        // バックグラウンド中ペインの backend セッションは backend_sessions に残るため上で網羅されるが、
        // 念のため明示的に保護する（生かしたまま隠れている）
        for pane in self.workspace.shelved_panes() {
            if let Some(name) = self.backend_sessions.get(&pane.id()) {
                protected.insert(name.clone());
            }
        }
        // 表示中ビューの元セッション・ラッパーも保護（足元を崩さない）
        for target in self.tmux_view_panes.values() {
            protected.insert(target.session.clone());
            if let Some(wrapper) = &target.wrapper {
                protected.insert(wrapper.clone());
            }
        }
        let socket = tako_core::tmux_backend::socket_name();
        tako_core::tmux_backend::cleanup_orphans(&socket, &protected)
    }

    fn tmux_tab_collapsed(&self, tab: TabId) -> bool {
        self.collapsed_tmux_tabs.contains(&tab)
    }

    fn set_tmux_tab_collapsed(&mut self, tab: TabId, collapsed: Option<bool>) {
        self.set_tmux_collapsed(tab, collapsed);
    }

    fn pinned_previews(&self) -> Vec<tako_control::PinnedView> {
        self.pinned_previews
            .iter()
            .map(|p| match p.target {
                PreviewTarget::Pane(id) => tako_control::PinnedView {
                    group: false,
                    id: id.as_u64(),
                    x: f32::from(p.pos.x),
                    y: f32::from(p.pos.y),
                },
                PreviewTarget::ClosedGroup(tab) => tako_control::PinnedView {
                    group: true,
                    id: tab.as_u64(),
                    x: f32::from(p.pos.x),
                    y: f32::from(p.pos.y),
                },
                PreviewTarget::TmuxWindow(pane, win) => tako_control::PinnedView {
                    group: false,
                    id: pane.as_u64() ^ ((win as u64) << 32),
                    x: f32::from(p.pos.x),
                    y: f32::from(p.pos.y),
                },
            })
            .collect()
    }

    fn set_pin_pane(&mut self, pane: PaneId, pinned: Option<bool>) {
        self.set_pin(PreviewTarget::Pane(pane), pinned);
    }

    fn set_pin_group(&mut self, tab: TabId, pinned: Option<bool>) {
        self.set_pin(PreviewTarget::ClosedGroup(tab), pinned);
    }

    fn reattach_backgrounded(&mut self, _pane: PaneId) {
        // セッションは terminals HashMap に残っている。再描画のみ必要
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

    fn backend_windows(&self, pane: PaneId) -> Option<Vec<tako_core::TmuxWindow>> {
        self.backend_windows.get(&pane).cloned()
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

    fn video_playback(&mut self, pane: PaneId, action: &str) -> Result<String, String> {
        let player = self
            .video_players
            .get_mut(&pane)
            .ok_or_else(|| "動画プレイヤーが起動していない".to_string())?;
        match action {
            "play" => player.play(),
            "pause" => player.pause(),
            "toggle" => player.toggle(),
            _ => return Err(format!("不明なアクション: {action}")),
        }
        let state_str = match player.state {
            video_player::PlaybackState::Playing => "playing",
            video_player::PlaybackState::Paused => "paused",
        };
        Ok(state_str.to_string())
    }

    fn video_seek(&mut self, pane: PaneId, seconds: f64) -> Result<f64, String> {
        let player = self
            .video_players
            .get_mut(&pane)
            .ok_or_else(|| "動画プレイヤーが起動していない".to_string())?;
        player.seek(seconds);
        Ok(player.current_time)
    }

    fn set_preview(
        &mut self,
        pane: PaneId,
        path: &str,
        mode: tako_control::protocol::PreviewModeWire,
    ) {
        let path = std::path::Path::new(path);
        let (state, raw) = preview::load_fast(path, preview::PreviewMode::from_wire(mode));
        if let Some(text) = raw {
            self.pending_highlights
                .push((pane, path.to_path_buf(), text));
        }
        self.previews.insert(pane, state);
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

    fn remote_start(
        &mut self,
        port: Option<u16>,
        no_tunnel: bool,
    ) -> Result<serde_json::Value, String> {
        // デーモンをバックグラウンドで fork 起動する
        tako_control::remote::spawn_daemon(port, no_tunnel)
    }

    fn remote_stop(&mut self) -> Result<serde_json::Value, String> {
        tako_control::remote::daemon_stop()
    }

    fn remote_status(&self) -> serde_json::Value {
        tako_control::remote::daemon_status()
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
        // インライン編集中は IME 確定文字列をインライン入力に振り分ける
        if let Some(ref mut edit) = self.inline_edit {
            if !text.is_empty() {
                edit.text.insert_str(edit.cursor, text);
                edit.cursor += text.len();
            }
            self.ime = None;
            cx.notify();
            return;
        }
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
        let ime_pane = self.ime_target();
        let origin = self.pane_cursor_origin(ime_pane, window)?;
        let cell = self.cell_size_for_pane(ime_pane)?;
        let x_offset = match self.ime.as_ref() {
            Some(ime) => {
                let start = clamp_ime_range_start(
                    range_utf16.start,
                    ime.text.encode_utf16().count(),
                    ime.selected_utf16.as_ref(),
                );
                let end = utf16_to_byte_offset(&ime.text, start);
                self.ime_prefix_width(&ime.text[..end], ime_pane, window)
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
        {
            let pane_ids: Vec<PaneId> = self.pane_font_sizes.keys().copied().collect();
            for pid in pane_ids {
                self.measure_pane_cell(pid, window);
            }
        }
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

        let drop_layout = layout.clone();
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

        // D&D 中のみ、各ペインにドロップ先オーバーレイを重ねる（FR-2.16.10 / FR-3.11）。
        // gpui 側のドラッグが外部要因（Esc 等）で消えたフレームでは出さない
        let drop_overlays: Vec<_> = self
            .drag_kind
            .filter(|_| cx.has_active_drag())
            .map(|kind| {
                drop_layout
                    .iter()
                    .map(|(id, rect)| self.render_drop_target(*id, *rect, kind, cx))
                    .collect()
            })
            .unwrap_or_default();

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

        let context_menu_overlay = self.render_context_menu(cx);
        // サイドバー tmux ビューのホバープレビュー（FR-2.16.13。マウス位置に実画面サムネイル）
        let hover_preview_overlay = self.render_hover_preview(window);
        // ピン留めされた常駐プレビュー（FR-2.16.15。アプリ内フローティングウィンドウ）
        let pinned_overlays = self.render_pinned_previews(cx);

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
                // 終了直前の構成を保存してから抜ける（Phase 5.5。セッションは残る = 永続化）
                this.save_layout();
                // persist ON（セッション生存）なら接続情報を残す: ソケットパス・トークンが
                // 再起動後も同一のため、既存クライアントがそのまま再接続できる。
                // persist OFF なら旧来通り片付け（死んだ接続先を CLI の候補に残さない）
                if !this.tmux_persist {
                    tako_control::discovery::cleanup(std::process::id());
                }
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
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                this.zoom_focused_pane(Self::FONT_SIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.zoom_focused_pane(-Self::FONT_SIZE_STEP, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetZoom, _, cx| this.reset_zoom_focused_pane(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all_preview(cx)))
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
                            .p(px(8.0))
                            .children(panes)
                            .children(border_handles)
                            .children(drop_overlays)
                            .children(ime_overlay),
                    )
                    .children(self.render_panel(cx)),
            )
            .children(self.render_drawer(cx))
            .child(self.render_status_bar(cx))
            .child(ime_registration)
            .children(context_menu_overlay)
            .children(hover_preview_overlay)
            .children(pinned_overlays)
    }
}

// ──────────────────────── グラフ描画 ────────────────────────

const LANE_W: f32 = 14.0;
const LINE_W: f32 = 2.0;
const DOT_R: f32 = 4.0;
const CURVE_STEPS: usize = 16;

/// canvas の paint コールバックから呼ぶグラフ 1 行分の描画
fn paint_graph_row(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    lines: &[tako_core::GraphLine],
    commit_lane: usize,
    commit_color: usize,
) {
    let ox = bounds.origin.x;
    let oy = bounds.origin.y;
    let h = bounds.size.height;
    let mid = h * 0.5;
    let lw = px(LINE_W);
    let half_lw = px(LINE_W / 2.0);

    for line in lines {
        match *line {
            tako_core::GraphLine::Vertical { lane, color_index } => {
                let x = ox + px(LANE_W / 2.0 + lane as f32 * LANE_W);
                window.paint_quad(fill(
                    Bounds::new(point(x - half_lw, oy), size(lw, h)),
                    hsla(tako_core::GRAPH_PALETTE[color_index]),
                ));
            }
            tako_core::GraphLine::VerticalTop { lane, color_index } => {
                let x = ox + px(LANE_W / 2.0 + lane as f32 * LANE_W);
                window.paint_quad(fill(
                    Bounds::new(point(x - half_lw, oy), size(lw, mid)),
                    hsla(tako_core::GRAPH_PALETTE[color_index]),
                ));
            }
            tako_core::GraphLine::VerticalBottom { lane, color_index } => {
                let x = ox + px(LANE_W / 2.0 + lane as f32 * LANE_W);
                window.paint_quad(fill(
                    Bounds::new(point(x - half_lw, oy + mid), size(lw, h - mid)),
                    hsla(tako_core::GRAPH_PALETTE[color_index]),
                ));
            }
            tako_core::GraphLine::CurveDown {
                from_lane,
                to_lane,
                color_index,
            } => {
                let x1 = ox + px(LANE_W / 2.0 + from_lane as f32 * LANE_W);
                let x2 = ox + px(LANE_W / 2.0 + to_lane as f32 * LANE_W);
                let y1 = oy + mid;
                let y2 = oy + h;
                let color = hsla(tako_core::GRAPH_PALETTE[color_index]);
                let dot = px(LINE_W + 0.5);
                let half_dot = dot * 0.5;
                for s in 0..=CURVE_STEPS {
                    let t = s as f32 / CURVE_STEPS as f32;
                    let st = t * t * (3.0 - 2.0 * t);
                    let x = x1 + (x2 - x1) * st;
                    let y = y1 + (y2 - y1) * t;
                    window.paint_quad(fill(
                        Bounds::new(point(x - half_dot, y - half_dot), size(dot, dot)),
                        color,
                    ));
                }
            }
        }
    }

    // コミットドット（円）
    let dot_r = px(DOT_R);
    let dot_d = dot_r + dot_r;
    let cx_pos = ox + px(LANE_W / 2.0 + commit_lane as f32 * LANE_W);
    let cy_pos = oy + mid;
    window.paint_quad(quad(
        Bounds::new(point(cx_pos - dot_r, cy_pos - dot_r), size(dot_d, dot_d)),
        dot_r,
        hsla(tako_core::GRAPH_PALETTE[commit_color]),
        px(0.),
        Hsla::default(),
        BorderStyle::default(),
    ));
}

/// background thread で git データを取得する（2 秒ポーリング用）
fn fetch_git_data(cwd: &std::path::Path, selected_commit: Option<&str>) -> Option<GitPanelData> {
    let repo = tako_core::git::repo_root(cwd)?;
    let commits = tako_core::git::log_commits(&repo, 200);
    let graph = tako_core::git::compute_graph_layout(&commits);
    let branches = tako_core::git::list_branches(&repo);
    let status = tako_core::git::status(&repo);
    let diff_files = if let Some(hash) = selected_commit {
        tako_core::git::diff(&repo, &tako_core::DiffTarget::Commit(hash.to_string()))
    } else {
        let mut files = tako_core::git::diff(&repo, &tako_core::DiffTarget::Unstaged);
        let staged = tako_core::git::diff(&repo, &tako_core::DiffTarget::Staged);
        files.extend(staged);
        files
    };
    Some(GitPanelData {
        repo_root: repo.display().to_string(),
        branch: status.branch.clone(),
        upstream: status.upstream.clone(),
        commits,
        branches,
        status: status.entries,
        diff_files,
        graph,
    })
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
    application()
        .with_assets(file_icons::TakoAssets)
        .run(|cx: &mut App| {
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
                    let bounds =
                        Bounds::new(point(px(f.x), px(f.y)), size(px(f.width), px(f.height)));
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
                                        run.fg == theme.red
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
            check(status == 200 && tool_count == 43, "MCP tools/list は 43 ツール");

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

            // 47. ペインの × ボタン = kill（dispatch 共有経路）。split で増やして × 相当の
            //     操作でアクティブタブから片付き、ターミナル（プロセス）も破棄され、バックグラウンドにも
            //     残らないこと。タブの × と挙動を統一し、紐づく tmux セッションも
            //     remove_pane が kill するため管理外 / orphan に残らない
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
                        && !app.workspace.shelved_panes().iter().any(|p| p.id() == target)
                        && !app.terminals.contains_key(&target)
                })
                .unwrap_or(false);
            check(close_button_ok, "ペインの × ボタンで kill（dispatch 経由）");

            // 47b. ペインの ー ボタン = バックグラウンドへバックグラウンド（dispatch 共有経路）。プロセス
            //      （ターミナル）は生かしたまま、ツリーから外れて shelved へ移ること（FR-2.15.1）
            type_text(any, cx, &format!("{cli} split --right >/dev/null"), true);
            wait(cx, 1500).await;
            let shelve_button_ok = window
                .update(cx, |app, _, cx| {
                    let before = app.workspace.active_tab().tree().len();
                    let target = app.focused_pane();
                    app.background_pane_button(target, cx);
                    let tree = app.workspace.active_tab().tree();
                    before == 2
                        && tree.len() == 1
                        && !tree.contains(target)
                        && app.workspace.shelved_panes().iter().any(|p| p.id() == target)
                        && app.terminals.contains_key(&target)
                })
                .unwrap_or(false);
            check(shelve_button_ok, "ペインの ー ボタンでバックグラウンド（dispatch 経由）");

            // 47c. バックグラウンドドロワーを開き、横並びの実画面プレビューが描画されること（FR-2.15）。
            //      47b でバックグラウンドしたペインが残っており、render_drawer がレイアウト含め panic しない
            //      （panic すれば自己テスト全体が落ちる）。検証後はドロワーを閉じて後続へ影響させない
            window
                .update(cx, |app, _, cx| {
                    app.drawer_visible = true;
                    cx.notify();
                })
                .ok();
            wait(cx, 400).await;
            let drawer_ok = window
                .update(cx, |app, _, cx| {
                    let ok = app.drawer_visible && !app.workspace.shelved_panes().is_empty();
                    app.drawer_visible = false;
                    cx.notify();
                    ok
                })
                .unwrap_or(false);
            check(drawer_ok, "バックグラウンドドロワーがバックグラウンドプレビューを描画");

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
                            tab: None,
                            direction: None,
                            ratio: None,
                            command: None,
                            cwd: None,
                            focus: None,
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
                    for (pane, data) in std::mem::take(&mut app.pending_writes) {
                        if let Some(session) = app.terminals.get(&pane) {
                            session.write(data);
                        }
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
                                direction: None,
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
                            tab: None,
                            direction: None,
                            ratio: None,
                            command: None,
                            cwd: Some("/private/tmp".into()),
                            focus: None,
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("split は成功する");
                    // 直接 dispatch のためセッション起動依頼はここで処理（項目 56 と同じ）
                    for (pane, options) in std::mem::take(&mut app.pending_attach) {
                        app.spawn_session(pane, options, cx)
                            .expect("一時ペインの PTY 起動は成功する");
                    }
                    for (pane, data) in std::mem::take(&mut app.pending_writes) {
                        if let Some(session) = app.terminals.get(&pane) {
                            session.write(data);
                        }
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

            // 68. D&D の同等操作（開発不変条件。FR-2.16.10 / FR-3.11 / FR-1.10）:
            //     UI のドロップと同じ dispatch 経路（TmuxOpen / OpenFile direction /
            //     MovePane target）を機械検証する。tmux 系は専用 -L ソケットで隔離
            if has_tmux {
                let dnd_sock = format!("tako-selftest-dnd-{}", std::process::id());
                let created = std::process::Command::new("tmux")
                    .args(["-L", &dnd_sock, "new-session", "-d", "-s", "dnd-src"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                check(created, "D&D 用 tmux セッション作成");
                wait(cx, 300).await;
                let opened_pane = window
                    .update(cx, |app, _, cx| {
                        let base = app.focused_pane().as_u64();
                        let opened = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::TmuxOpen {
                                socket: Some(dnd_sock.clone()),
                                session: "dnd-src".into(),
                                window: None,
                                pane: Some(base),
                                direction: Some(tako_control::protocol::Direction::Down),
                            },
                            PaneOrigin::Cli,
                        )
                        .expect("tmux open は成功する");
                        // セッション起動を伴う直接 dispatch（項目 56 / 67 と同じ後処理）
                        for (pane, options) in std::mem::take(&mut app.pending_attach) {
                            app.spawn_session(pane, options, cx)
                                .expect("取り込みペインの PTY 起動は成功する");
                        }
                        for (pane, data) in std::mem::take(&mut app.pending_writes) {
                            if let Some(session) = app.terminals.get(&pane) {
                                session.write(data);
                            }
                        }
                        opened["pane"].as_u64().expect("pane が返る")
                    })
                    .unwrap_or(0);
                // attach クライアントが実際にセッションへ繋がる（list-clients が非空になる）
                let mut attached = false;
                for _ in 0..25 {
                    wait(cx, 400).await;
                    attached = std::process::Command::new("tmux")
                        .args(["-L", &dnd_sock, "list-clients"])
                        .output()
                        .map(|o| o.status.success() && !o.stdout.is_empty())
                        .unwrap_or(false);
                    if attached {
                        break;
                    }
                }
                check(attached, "tmux open: 分割ペインの attach クライアントが繋がる");
                // 存在しないセッション名は分割前に弾かれる（空ペインが生えない）
                let rejected = window
                    .update(cx, |app, _, _| {
                        let before = app.workspace.active_tab().tree().len();
                        let base = app.focused_pane().as_u64();
                        let err = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::TmuxOpen {
                                socket: Some(dnd_sock.clone()),
                                session: "no-such-session".into(),
                                window: None,
                                pane: Some(base),
                                direction: None,
                            },
                            PaneOrigin::Cli,
                        )
                        .is_err();
                        err && app.workspace.active_tab().tree().len() == before
                    })
                    .unwrap_or(false);
                check(rejected, "tmux open: 存在しないセッションは分割前に弾く");
                // 取り込みペインを閉じてもセッション側は残る（kill ではない）
                let _ = window.update(cx, |app, _, cx| {
                    let _ = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Close {
                            pane: Some(opened_pane),
                        },
                        PaneOrigin::Cli,
                    );
                    cx.notify();
                });
                wait(cx, 800).await;
                let survives = std::process::Command::new("tmux")
                    .args(["-L", &dnd_sock, "has-session", "-t", "=dnd-src"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                check(survives, "tmux open: ペイン close でもセッションは残る");
                let _ = std::process::Command::new("tmux")
                    .args(["-L", &dnd_sock, "kill-server"])
                    .status();
            } else {
                eprintln!("（tmux 不在のため項目 68 をスキップ）");
            }

            // 68b. OpenFile の direction（ファイル D&D のドロップ位置。FR-3.11）:
            //      既存プレビューがあっても再利用せず指定方向に分割して開く
            let dnd_dir =
                std::env::temp_dir().join(format!("tako-selftest-dnd-file-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dnd_dir);
            std::fs::create_dir_all(&dnd_dir).expect("一時ディレクトリを作れる");
            std::fs::write(dnd_dir.join("a.rs"), "fn main() {}\n").unwrap();
            let open_direction_ok = window
                .update(cx, |app, _, _| {
                    let base = app.focused_pane().as_u64();
                    let path = dnd_dir.join("a.rs").display().to_string();
                    let open = |app: &mut TakoApp, direction| {
                        tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::OpenFile {
                                pane: Some(base),
                                path: path.clone(),
                                mode: None,
                                direction,
                            },
                            PaneOrigin::Cli,
                        )
                        .expect("open_file は成功する")
                    };
                    let first = open(app, None)["pane"].as_u64().expect("pane が返る");
                    let second =
                        open(app, Some(tako_control::protocol::Direction::Down));
                    let split_new = second["pane"].as_u64() != Some(first)
                        && second["created"].as_bool() == Some(true)
                        && app.previews.len() == 2;
                    // 後片付け: プレビュー 2 枚を閉じる
                    for pane in [first, second["pane"].as_u64().unwrap_or(0)] {
                        let _ = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Close { pane: Some(pane) },
                            PaneOrigin::Cli,
                        );
                    }
                    split_new && app.previews.is_empty()
                })
                .unwrap_or(false);
            check(
                open_direction_ok,
                "OpenFile direction は再利用せず指定方向に開く（ファイル D&D 同等）",
            );
            let _ = std::fs::remove_dir_all(&dnd_dir);

            // 68c. MovePane の target + direction（タイトルバー D&D のペイン移動。FR-1.10）:
            //      [base | p2] から base を p2 の下へ → 縦分割（p2 上 / base 下・全幅）
            let move_ok = window
                .update(cx, |app, _, cx| {
                    let base = app.focused_pane().as_u64();
                    let split = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Split {
                            pane: Some(base),
                            tab: None,
                            direction: None,
                            ratio: None,
                            command: None,
                            cwd: None,
                            focus: None,
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("split は成功する");
                    for (pane, options) in std::mem::take(&mut app.pending_attach) {
                        app.spawn_session(pane, options, cx)
                            .expect("一時ペインの PTY 起動は成功する");
                    }
                    for (pane, data) in std::mem::take(&mut app.pending_writes) {
                        if let Some(session) = app.terminals.get(&pane) {
                            session.write(data);
                        }
                    }
                    let p2 = split["pane"].as_u64().expect("pane が返る");
                    tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::MovePane {
                            pane: Some(base),
                            tab: None,
                            target: Some(p2),
                            direction: Some(tako_control::protocol::Direction::Down),
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("move-pane --target は成功する");
                    let rects = app.workspace.active_tab().tree().layout(Rect::UNIT);
                    let rect_of = |raw: u64| {
                        rects
                            .iter()
                            .find(|(p, _)| p.as_u64() == raw)
                            .map(|(_, r)| *r)
                    };
                    // base が p2 の「直下」（同列・同幅の縦分割）に入る。タブに他の
                    // ペインが残っていても成り立つ相対条件で見る
                    let moved = match (rect_of(p2), rect_of(base)) {
                        (Some(top), Some(bottom)) => {
                            top.y < bottom.y
                                && (top.x - bottom.x).abs() < 1e-5
                                && (top.width - bottom.width).abs() < 1e-5
                        }
                        _ => false,
                    };
                    // 後片付け: 一時ペインを閉じて 1 ペイン構成へ戻す
                    let _ = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Close { pane: Some(p2) },
                        PaneOrigin::Cli,
                    );
                    moved
                })
                .unwrap_or(false);
            check(
                move_ok,
                "MovePane target+direction の挿し直し（タイトルバー D&D 同等）",
            );

            // 69. 画像プレビュー（FR-3.10）: dispatch OpenFile で PNG を開き、
            //     Image モードで表示される。拡張子ベースの自動判定が効く。
            //     list にも preview.mode="image" として公開される
            let img_dir =
                std::env::temp_dir().join(format!("tako-selftest-img-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&img_dir);
            std::fs::create_dir_all(&img_dir).expect("一時ディレクトリを作れる");
            // 最小の有効な PNG（1x1 透明ピクセル）
            let png_bytes: Vec<u8> = vec![
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49,
                0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
                0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44,
                0x41, 0x54, 0x78, 0x9C, 0x62, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE5, 0x27,
                0xDE, 0xFC, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60,
                0x82,
            ];
            std::fs::write(img_dir.join("test.png"), &png_bytes).unwrap();
            // JPEG のダミー（マジックバイトだけ。デコードは GPUI 側でここでは読み込みのみ）
            std::fs::write(img_dir.join("photo.jpg"), [0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
            let img_ok = window
                .update(cx, |app, _, _cx| {
                    let base = app.focused_pane().as_u64();
                    let open = |app: &mut TakoApp, path: String| {
                        tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::OpenFile {
                                pane: Some(base),
                                path,
                                mode: None,
                                direction: None,
                            },
                            PaneOrigin::Cli,
                        )
                    };
                    // PNG を開く
                    let r =
                        open(app, img_dir.join("test.png").display().to_string()).expect("PNG を開ける");
                    let pane_id = r["pane"].as_u64().expect("pane が返る");
                    let png_ok = r["mode"].as_str() == Some("image")
                        && app
                            .previews
                            .iter()
                            .any(|(pid, p)| {
                                pid.as_u64() == pane_id
                                    && matches!(
                                        &p.content,
                                        preview::PreviewContent::Image(d)
                                            if d.format == preview::ImageFileFormat::Png
                                    )
                            });
                    // JPEG を同じプレビューペインで再利用して開く
                    let r2 =
                        open(app, img_dir.join("photo.jpg").display().to_string()).expect("JPEG を開ける");
                    let jpg_ok = r2["mode"].as_str() == Some("image")
                        && r2["pane"].as_u64() == Some(pane_id); // 再利用
                    // list に preview.mode="image" が見える
                    let list =
                        tako_control::dispatch(app, tako_control::protocol::Request::List, PaneOrigin::Cli)
                            .expect("list は成功する");
                    let has_image = list.to_string().contains("\"image\"");
                    // 後片付け
                    let _ = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Close {
                            pane: Some(pane_id),
                        },
                        PaneOrigin::Cli,
                    );
                    png_ok && jpg_ok && has_image
                })
                .unwrap_or(false);
            check(img_ok, "画像プレビュー（FR-3.10。PNG / JPEG の OpenFile と list 公開）");
            let _ = std::fs::remove_dir_all(&img_dir);

            // 70. PDF プレビュー（FR-3.4 macOS）: dispatch OpenFile で PDF を開き、
            //     Pdf モードで Core Graphics レンダリングされたページが表示される
            #[cfg(target_os = "macos")]
            {
                let pdf_dir =
                    std::env::temp_dir().join(format!("tako-selftest-pdf-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&pdf_dir);
                std::fs::create_dir_all(&pdf_dir).expect("一時ディレクトリを作れる");
                // 最小の有効な PDF（1 ページ・空白）
                let pdf_content = b"%PDF-1.0\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj 2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj 3 0 obj<</Type/Page/MediaBox[0 0 612 792]/Parent 2 0 R>>endobj\nxref\n0 4\n0000000000 65535 f \n0000000009 00065 n \n0000000058 00000 n \n0000000115 00000 n \ntrailer<</Size 4/Root 1 0 R>>\nstartxref\n190\n%%EOF";
                std::fs::write(pdf_dir.join("test.pdf"), pdf_content).unwrap();
                let pdf_ok = window
                    .update(cx, |app, _, _cx| {
                        let base = app.focused_pane().as_u64();
                        let r = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::OpenFile {
                                pane: Some(base),
                                path: pdf_dir.join("test.pdf").display().to_string(),
                                mode: None,
                                direction: None,
                            },
                            PaneOrigin::Cli,
                        )
                        .expect("PDF を開ける");
                        let pane_id = r["pane"].as_u64().expect("pane が返る");
                        let mode_ok = r["mode"].as_str() == Some("pdf");
                        let content_ok = app
                            .previews
                            .iter()
                            .any(|(pid, p)| {
                                pid.as_u64() == pane_id
                                    && matches!(
                                        &p.content,
                                        preview::PreviewContent::Pdf(d)
                                            if d.total_pages == 1
                                                && d.pages.len() == 1
                                                && !d.pages[0].is_empty()
                                    )
                            });
                        let _ = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Close {
                                pane: Some(pane_id),
                            },
                            PaneOrigin::Cli,
                        );
                        mode_ok && content_ok
                    })
                    .unwrap_or(false);
                check(pdf_ok, "PDF プレビュー（FR-3.4。Core Graphics レンダリング）");
                let _ = std::fs::remove_dir_all(&pdf_dir);
            }

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
    use super::{accumulate_scroll, drop_zone, zone_to_direction, DropZone};

    #[test]
    fn ドロップゾーンは象限と中央を判定する() {
        // 4 象限（対角線分割。tmux ドラッグ = center_allowed なし）
        assert_eq!(drop_zone(0.1, 0.5, false), DropZone::Left);
        assert_eq!(drop_zone(0.9, 0.5, false), DropZone::Right);
        assert_eq!(drop_zone(0.5, 0.1, false), DropZone::Up);
        assert_eq!(drop_zone(0.5, 0.9, false), DropZone::Down);
        // 中央でも center_allowed なしなら最寄り辺に落ちる
        assert!(matches!(
            drop_zone(0.5, 0.45, false),
            DropZone::Up | DropZone::Left | DropZone::Right | DropZone::Down
        ));
        assert_ne!(drop_zone(0.5, 0.45, false), DropZone::Center);
        // ファイルドラッグは中央 40% が Center（= 再利用セマンティクス）
        assert_eq!(drop_zone(0.5, 0.5, true), DropZone::Center);
        assert_eq!(drop_zone(0.31, 0.69, true), DropZone::Center);
        assert_eq!(drop_zone(0.1, 0.5, true), DropZone::Left);
        assert_eq!(drop_zone(0.5, 0.75, true), DropZone::Down);
        // ゾーン → 方向の写し（Center は呼び出し側で direction なしに落とす）
        use tako_control::protocol::Direction;
        assert_eq!(zone_to_direction(DropZone::Left), Direction::Left);
        assert_eq!(zone_to_direction(DropZone::Down), Direction::Down);
    }

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

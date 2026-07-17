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
mod drawer;
mod file_icons;
mod filetree;
mod keybindings;
mod overlays;
mod preview;
mod preview_render;
mod preview_watch;
mod right_panel;
mod sidebar;
mod status_bar;
mod tab_bar;
mod update_checker;
mod video_player;
mod webview;

use keybindings::*;
use preview_render::{PreviewImageCache, PreviewImageCacheEntryKey};

use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::{
    canvas, div, fill, point, prelude::*, px, quad, relative, size, svg, App, BorderStyle, Bounds,
    BoxShadow, ClipboardItem, Context, CursorStyle, DragMoveEvent, ElementInputHandler,
    EntityInputHandler, ExternalPaths, FocusHandle, Font, FontStyle, FontWeight, HighlightStyle,
    Hsla, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Rgba, ScrollDelta, ScrollWheelEvent, SharedString, Size, StrikethroughStyle, StyledText,
    TextLayout, TextRun, TextStyle, UTF16Selection, UnderlineStyle, Window, WindowBounds,
    WindowOptions,
};
use gpui_platform::application;
use tako_control::{
    IncomingRequest, IpcServer, McpServer, PreviewHost, RemoteHost, SessionHost, SystemHost,
    TmuxHost, UiStateHost, WebViewHost, WorkspaceHost,
};
use tako_core::{
    ratio_for_position, AgentMetrics, CommandState, Pane, PaneId, PaneOrigin, Rect, SelectionKind,
    SessionNotice, SpawnOptions, SplitAxis, SplitDirection, TabId, TerminalSession, Theme,
    TitleSource, Workspace, WorkspaceError,
};

/// 新規セッションの初期グリッド。最初の render で実寸へリサイズされる
const INITIAL_COLS: usize = 80;
const INITIAL_ROWS: usize = 24;

/// 実行中 Claude Code とペインの対応を layout.json へ反映する間隔。
/// 外部 CLI を呼ぶため描画ループとは分離し、成功時だけ保存キャッシュを更新する。
const CLAUDE_SESSION_SCAN_INTERVAL: Duration = Duration::from_secs(5);

/// 復元時に新しいシェルへ投入する Claude resume コマンドを安全条件つきで組み立てる。
/// backend 生存時はプロセスごと再 attach するため、二重起動を避けて None。
fn claude_resume_command(
    backend_alive: bool,
    session_id: Option<&str>,
    transcript_exists: bool,
) -> Option<Vec<u8>> {
    let session_id = (!backend_alive && transcript_exists)
        .then_some(session_id)
        .flatten()
        .filter(|id| tako_control::transcript::is_valid_session_id(id))?;
    Some(format!("claude --resume {session_id}\r").into_bytes())
}

/// タブバーの高さ（px）
const TAB_BAR_HEIGHT: f32 = 44.0;
/// ペイン枠線の太さ（px）
const PANE_BORDER: f32 = 1.0;
/// ペイン内側の余白（px。デザインスペック: 12–14px content padding）
const PANE_PADDING: f32 = 10.0;
/// キーボードリサイズ 1 回あたりの比率変化
const RESIZE_STEP: f32 = 0.05;
/// ペイン境界のドラッグ判定/カーソル変更の当たり幅（px。仕切り線を中心に左右各 BORDER_HANDLE/2）
const BORDER_HANDLE: f32 = 8.0;

/// ドラッグ選択自動スクロール: ペイン上下端からこの範囲内でスクロールを開始する（px）
const DRAG_SCROLL_MARGIN: f32 = 40.0;
/// ドラッグ選択自動スクロールのタイマー間隔（ms）
const DRAG_SCROLL_INTERVAL_MS: u64 = 30;
/// ドラッグ選択自動スクロールの最小速度（行/秒）
const DRAG_SCROLL_MIN_SPEED: f32 = 2.0;
/// ドラッグ選択自動スクロールの最大速度（行/秒）
const DRAG_SCROLL_MAX_SPEED: f32 = 30.0;

/// 速度係数（0.0..1.0）を 1 ティック分のスクロール行数（正）に変換する
fn drag_scroll_delta(factor_abs: f32) -> f32 {
    let speed =
        DRAG_SCROLL_MIN_SPEED + (DRAG_SCROLL_MAX_SPEED - DRAG_SCROLL_MIN_SPEED) * factor_abs;
    speed * (DRAG_SCROLL_INTERVAL_MS as f32 / 1000.0)
}

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
///
/// #159 でローカルミラー方式へ刷新: スクロール開始時に tmux 履歴を capture して
/// [`ScrollMirror`] としてローカルに持ち、以降の描画は完全ローカル（直接ペインと
/// 同じピクセル単位のサブライン描画）。copy-mode には入らない（旧方式の
/// ① 行単位 ② tmux 往復レイテンシ ③ キー飲まれ、の 3 制約を構造的に解消）。
/// マウス要求アプリ（vim / claude 等）への生 SGR 転送は従来どおり
struct ScrollCtl {
    /// 解決済みのスクロール実体（None = 未解決。初回ロードで解決する）
    target: Option<tako_core::scroll::ScrollTarget>,
    /// 対象ペインのアプリがマウスを要求しているか（true なら生 SGR 転送に任せる）。
    /// None = 未解決（初回ロードで判定）
    wants_mouse: Option<bool>,
    /// スクロールバック表示のローカルミラー（None = 最下部・ライブ表示）
    mirror: Option<tako_core::scroll_mirror::ScrollMirror>,
    /// 最後に知った tmux 履歴行数（スクロールバー表示用。ロード時に更新）
    known_history: usize,
    /// capture / 解決の実行中（完了時に pending を反映して必要なら再ポンプ）
    loading: bool,
    /// 解決・ロード完了待ちのホイール蓄積（行小数。正 = 遡る）
    pending_rows: f32,
    last_activity: std::time::Instant,
    /// 増分追記（新規出力の押し出し行回収）の間隔制御
    last_refresh: std::time::Instant,
    /// 直近のホイール座標（マウス要求アプリと判明したとき生 SGR へ流す用）
    last_cell: (usize, usize),
    /// 内側アプリのレポート形式が SGR か（`#{mouse_sgr_flag}`。false = X10 形式）
    wants_sgr: bool,
    /// tmux 直接注入（send-keys -H。#167）待ちのホイールイベント数（正 = 上方向）
    pending_wheel: i32,
    /// send-keys 実行中フラグ（実行中は `pending_wheel` に溜めて直列化する）
    wheel_sending: bool,
}

impl Default for ScrollCtl {
    fn default() -> Self {
        Self {
            target: None,
            wants_mouse: None,
            mirror: None,
            known_history: 0,
            loading: false,
            pending_rows: 0.0,
            last_activity: std::time::Instant::now(),
            last_refresh: std::time::Instant::now(),
            last_cell: (0, 0),
            wants_sgr: true,
            pending_wheel: 0,
            wheel_sending: false,
        }
    }
}

impl ScrollCtl {
    /// ミラースクロール表示中か（0 より上を見ている）
    fn mirror_scrolling(&self) -> bool {
        self.mirror
            .as_ref()
            .is_some_and(|m| m.effective_position() > 0.0)
    }
}

/// 左サイドバー（ファイルツリー）の最小幅（px。FR-3.1。ドラッグで可変。
/// 既定幅は settings::default_sidebar_width() = 244）
const SIDEBAR_MIN_WIDTH: f32 = 120.0;

/// 右サイドバー（情報パネル）の既定幅・最小幅（px。ドラッグで可変）
const PANEL_DEFAULT_WIDTH: f32 = 320.0;
const PANEL_MIN_WIDTH: f32 = 220.0;

/// ペイン上部タイトルバーの高さ（px。デザインスペック: 30px）
const PANE_TITLE_BAR: f32 = 32.0;

/// 下部ステータスバーの高さ（px。FR-2.16.4。Zed / VSCode 風）
const STATUS_BAR_HEIGHT: f32 = 32.0;

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
    /// オーケストレーター中心ビュー（#217 カンプの「orch」タブ。master とその
    /// ワーカーツリー・メトリクスを俯瞰する）
    Orch,
    Git,
}

/// プレビューヘッダから開くナビゲーションドロップダウン（Issue #232）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewNavigationPanel {
    Outline,
    Pages,
}

/// claude TUI へのプロンプト送信フローの状態（Issue #32 送達確認ループ）
#[derive(Debug)]
enum PromptFlowState {
    /// alt_screen 遷移待ち（claude TUI の起動待ち。spawn / await_prompt 経路のみ）
    WaitAltScreen,
    /// 送信可能待ち: 信頼ダイアログが出ていれば承諾し、入力欄（プロンプト記号）表示で貼り付ける。
    /// 信頼ダイアログも選択カーソルに同じプロンプト記号を含むため、ダイアログ判定を先に行う
    WaitPromptReady,
    /// 貼り付け済み、入力欄への反映待ち。反映確認後に送信の Enter を
    /// 貼り付けと分離した単独キーとして送る（次 tick = 500ms 以上の遅延）
    WaitTextInInput,
    /// Enter 送信済み。入力欄が空へ戻った（= 送信された）ことを検証し、
    /// 残っていれば Enter を単独再送する
    VerifySubmitted,
    /// 完了
    Done,
}

/// claude TUI へのプロンプト送達ステートマシン（500ms tick で `drive_prompt_flows` が駆動）
#[derive(Debug)]
struct PromptFlow {
    pane: PaneId,
    prompt: String,
    state: PromptFlowState,
    created_at: std::time::Instant,
    /// 現在のステートに遷移した時刻（ステート内タイムアウト用）
    state_entered_at: std::time::Instant,
    /// 信頼ダイアログを承諾した回数（無限承諾ループ防止。上限 3）
    trust_accepts: u8,
    /// 入力欄残留に対する Enter 単独再送の残り回数
    enter_retries_left: u8,
    /// true = claude TUI の起動（alt_screen + プロンプト記号）を待つ（spawn / await_prompt）。
    /// false = 現画面へ即貼り付けの汎用送信（TUI でなければ 2 秒待って貼る）
    wait_tui: bool,
    /// Enter 単独送達モード（Issue #95）: 貼り付けせず Enter を送り、入力欄が
    /// 空へ戻るまで単独再送する（入力欄に残留したテキストの送信代行）
    enter_only: bool,
    /// enter_only の残留判定基準: Enter 送信時点の入力欄内容。検証時に
    /// 同じ内容が残っていれば未送達とみなし Enter を再送する
    baseline: Option<String>,
}

impl PromptFlow {
    fn new(pane: PaneId, prompt: String, wait_tui: bool) -> Self {
        let now = std::time::Instant::now();
        Self {
            pane,
            // 送信の Enter は分離して送るため末尾改行は落とす
            prompt: prompt.trim_end_matches(['\n', '\r']).to_string(),
            state: if wait_tui {
                PromptFlowState::WaitAltScreen
            } else {
                PromptFlowState::WaitPromptReady
            },
            created_at: now,
            state_entered_at: now,
            trust_accepts: 0,
            enter_retries_left: 4,
            wait_tui,
            enter_only: false,
            baseline: None,
        }
    }

    /// Enter 単独送達フロー（Issue #95）: dispatch の Enter 単独送信
    /// （text が空 / 改行のみ）用。信頼ダイアログ処理 → Enter → 空検証 + 再送
    fn new_enter_only(pane: PaneId) -> Self {
        let mut flow = Self::new(pane, String::new(), false);
        flow.enter_only = true;
        flow
    }

    /// 人間の Enter の送達検証フロー（Issue #95）: Enter は handle_key で書き込み済み
    /// のため検証（VerifySubmitted）から開始する。`baseline` は Enter 書き込み直前の
    /// 入力欄内容（同じ内容が残っていれば未送達 = Enter を単独再送）
    fn new_enter_verify(pane: PaneId, baseline: String) -> Self {
        let mut flow = Self::new(pane, String::new(), false);
        flow.enter_only = true;
        flow.baseline = Some(baseline);
        flow.state = PromptFlowState::VerifySubmitted;
        flow
    }
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

/// プレビューライブリロード（Issue #233）の起動時の有効判定。
/// `TAKO_PREVIEW_RELOAD=0|false|off` は設定ファイルより優先して無効化する。
fn initial_preview_reload() -> bool {
    if matches!(
        std::env::var("TAKO_PREVIEW_RELOAD").ok().as_deref(),
        Some("0" | "false" | "off")
    ) {
        return false;
    }
    tako_control::settings::load().preview_live_reload
}

/// デコード済みプレビュー画像キャッシュの起動時予算（Issue #258）。
fn initial_preview_cache_budget() -> u64 {
    let max_mb = tako_control::settings::load().preview_cache_max_mb;
    tako_core::preview_cache_bytes(max_mb)
        .unwrap_or(tako_core::PREVIEW_CACHE_DEFAULT_MB * 1024 * 1024)
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

/// ペインが閉じられる理由（Issue #30）。バックエンドセッションの kill と
/// layout.json の削除は**ユーザー / AI の明示操作に限る**。PTY 子プロセスの死
/// （シェル exit・tmux クライアント死）でセッションやレイアウトを道連れにしない:
/// tmux サーバー側の異常（サーバー死・クライアント kick）で全ペインの PTY が
/// 一斉終了したとき、旧実装は「明示 close」と同じ経路でセッションを kill し
/// layout.json も削除していた（2026-07-03 実機で全タブ消失）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseReason {
    /// × ボタン・cmd+W・CLI / MCP の close 等、ユーザー / AI の明示操作
    Explicit,
    /// PTY 子プロセスの終了（SessionNotice::Exited）
    Exited,
}

/// persist 診断ログ（Issue #30）: `<data_dir>/persist.log` へ追記 + stderr へも出す
/// （ターミナル起動時に見える）。Dock 起動の .app では stderr が届かないため、
/// 「再起動でタブが消えた」原因をファイルで追えるようにする。
/// セルフテスト中はユーザーの persist.log を汚さない。
/// pid を付与するのは、複数インスタンス（本番 + 実験起動）のログが同じファイルに
/// 混ざったとき、どの行がどの起動のものかを事後調査で切り分けるため（#177）
fn persist_diag(msg: &str) {
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        return;
    }
    let msg = format!("{msg} [pid {}]", std::process::id());
    eprintln!("persist: {msg}");
    tako_control::diag::persist_log(&msg);
}

/// 復元強奪ガード（#177）: これから復元 attach しようとする tmux セッションに
/// **生きた別 tako-app 配下のクライアント**が attach 中なら、そのセッション群は
/// 別インスタンスが表示中なのでセカンダリ降格の理由を返す。
///
/// 多重起動ガード（#113）は control.json（discovery）だけを見るため、
/// `TAKO_DISCOVERY_DIR` を隔離した起動や control.json の消失で盲目になる
/// （実機 2026-07-13: discovery だけ隔離した dev 起動がプライマリ判定 →
/// 本番 layout.json を復元 → `new-session -A -D` が稼働中インスタンスの
/// クライアント 13 本を強奪 → PTY 一斉死亡 + 縮退 layout 上書き）。
/// 判定材料を「守るべき資源そのもの」に置くことで隔離変数の組合せに依存しない。
///
/// 手動の `tmux attach`（ターミナル.app 等の配下）は tako-app 祖先を持たないため
/// 対象外（従来どおり -D で引き継ぐ）。正当な再起動では旧インスタンスの死亡と同時に
/// クライアントも死ぬ（PTY 閉鎖 → SIGHUP）ため、このガードは発動しない
fn foreign_client_guard() -> Option<String> {
    if !initial_tmux_persist() || !tako_core::tmux_backend::available() {
        return None;
    }
    let file = tako_control::layout::try_load().ok()?;
    let sessions: std::collections::HashSet<&str> = file.sessions().into_iter().collect();
    if sessions.is_empty() {
        return None;
    }
    let socket = tako_core::tmux_backend::socket_name();
    let clients: Vec<(u32, String)> = tako_core::tmux::list_client_pids(Some(&socket))
        .into_iter()
        .filter(|(_, session)| sessions.contains(session.as_str()))
        .collect();
    if clients.is_empty() {
        return None;
    }
    let parents = tako_control::agents::process_parent_map();
    for (pid, session) in &clients {
        if let Some(owner) = live_foreign_tako_ancestor(*pid, &parents) {
            return Some(format!(
                "復元対象の tmux セッション {session} に別の tako（pid {owner}）のクライアントが attach 中（表示中の資源は強奪しない）"
            ));
        }
    }
    None
}

/// `pid` の祖先（自身を含む）から「自プロセス以外の生きた tako-app」を探す。
/// tmux クライアントは tako-app が spawn した PTY の子プロセスなので、
/// 祖先を辿れば所有インスタンスに行き着く
fn live_foreign_tako_ancestor(pid: u32, parents: &HashMap<u32, u32>) -> Option<u32> {
    let me = std::process::id();
    let mut current = pid;
    // 祖先チェーンの上限（循環 ppid への防御。実際は数ホップで launchd に到達する）
    for _ in 0..32 {
        if current != me && tako_core::ports::is_live_tako_app(current) {
            return Some(current);
        }
        match parents.get(&current) {
            Some(&parent) if parent != 0 && parent != current => current = parent,
            _ => return None,
        }
    }
    None
}

/// 起動時 orphan cleanup の猶予（秒）。最終アクティビティがこれより新しい detached
/// セッションは自動 kill しない（Issue #113）。多重起動事故の時間スケール（分単位）より
/// 十分長く、真の残骸（前回クラッシュの取り残し）の滞留（時間〜日単位）より短い値
const CLEANUP_STARTUP_GRACE_SECS: u64 = 3600;

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

fn byte_to_utf16_offset(text: &str, byte_offset: usize) -> usize {
    let offset = snap_to_char_boundary(text, byte_offset.min(text.len()));
    text[..offset].chars().map(char::len_utf16).sum()
}

/// ドラッグ選択中の自動スクロール状態（#310）
struct DragScrollState {
    pane: PaneId,
    /// 正 = 過去方向（上）、負 = 未来方向（下）。絶対値が速度係数（0.0..1.0）
    speed_factor: f32,
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
    /// ドラッグ選択自動スクロールの状態（上下端到達時のスクロール方向。タイマー稼働中）
    drag_scroll: Option<DragScrollState>,
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
    /// dispatch 中に依頼された重量プレビュー（PDF / 動画）の background 読み込み
    /// （Issue #168。Loading 表示 → 完了時差し替え。GPUI の Context が要るため遅延実行）
    pending_preview_loads: Vec<(PaneId, std::path::PathBuf, preview::PreviewMode)>,
    /// 現在の GPUI ウィンドウが属する display の device scale。
    /// PDF の実ピクセル解像度を表示幅へ合わせるため render 冒頭で更新する。
    preview_device_scale: f32,
    /// ペインごとの最新 PDF 再ラスタライズ要求。リサイズ中は最新キーだけを残す。
    pending_pdf_rasters: HashMap<PaneId, PendingPdfRaster>,
    /// debounce / background ラスタライズループが稼働中のペイン。
    active_pdf_rasters: std::collections::HashSet<PaneId>,
    /// IME 変換中の未確定文字列（FR-1.9。None = 変換中でない）
    ime: Option<ImeComposition>,
    /// ドラッグ中のペイン境界（None = ドラッグしていない）
    dragging_border: Option<DragBorder>,
    /// スクロールバーをドラッグ中のペイン
    dragging_scrollbar: Option<PaneId>,
    /// スクロールバーにホバー中のペイン（表示維持 + サム強調。macOS 慣行）
    hovered_scrollbar: Option<PaneId>,
    /// ホイール行換算の端数持ち越し（accumulate_scroll）。ペイン close で破棄
    scroll_accum: HashMap<PaneId, f32>,
    /// バックエンド / ネスト tmux スクロールの UI 状態（ミラー + フェード表示）
    scroll_ctls: HashMap<PaneId, ScrollCtl>,
    /// フェード再描画・ミラー増分追従ティッカーの稼働中フラグ
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
    /// 左サイドバー幅（px。右端ハンドルのドラッグで可変。#307）
    sidebar_width: f32,
    /// 左サイドバー右端の境界をドラッグ中か（#307）
    dragging_sidebar: bool,
    /// 左サイドバーのファイルツリー（FR-3.1 / FR-3.7。cmd+B でトグル）
    filetree: filetree::FileTree,
    /// プレビューペイン（FR-3.2 / FR-3.3）。キーに居るペインはターミナルではなく
    /// ファイル内容（コードハイライト / Markdown レンダリング）を描画する
    previews: HashMap<PaneId, preview::PreviewState>,
    /// Markdown / PDF の目次または PDF ページ一覧を開いているペイン。
    preview_navigation_panel: Option<(PaneId, PreviewNavigationPanel)>,
    /// 表示中ファイルのライブリロード設定（core 状態を CLI / MCP と共有）。
    preview_reload: tako_core::PreviewReloadState,
    /// OS ネイティブのイベント駆動監視。生成不能時は None でライブリロードだけ無効化する。
    preview_file_watcher: Option<preview_watch::PreviewFileWatcher>,
    /// パス別の最終イベント時刻（300ms デバウンス）。イベントが無ければ空のまま。
    pending_preview_reloads: HashMap<std::path::PathBuf, std::time::Instant>,
    /// デバウンスタスクが稼働中のパス。連続 write でもタスクを増殖させない。
    active_preview_reloads: std::collections::HashSet<std::path::PathBuf>,
    /// 実読み込み中の (pane, path)。イベント頻度に比例した PDF 全ページ並行生成を防ぐ。
    active_preview_reload_jobs: std::collections::HashSet<(PaneId, std::path::PathBuf)>,
    /// background 完了の世代照合。後続 write より古い結果を表示へ適用しない。
    preview_reload_generations: HashMap<std::path::PathBuf, u64>,
    next_preview_reload_generation: u64,
    /// 隔離 E2E でデバウンス適用回数を実測する診断カウンタ。
    preview_reload_apply_count: u64,
    /// コードプレビューの編集セッション。未保存バッファは表示モードを OFF にしても保持する。
    preview_edits: HashMap<PaneId, preview::EditState>,
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
    /// セカンダリモード（Issue #113）: 別インスタンス（別プロセス or 同一プロセスの先行
    /// ウィンドウ）がプライマリとして生きている間 true。復元・layout.json への書き込み /
    /// 削除・tmux バックエンド・persist トグルを封じ、プライマリの作業を一切壊さない
    secondary: bool,
    /// 最後のタブの終了処理（remove_tab_with の LastTab 分岐）を通過済み（Issue #113:
    /// PTY の Exit / ChildExit 二重イベントによる「全ペイン終了」ログ + quit の重複発火防止。
    /// #103: on_app_quit の layout 保存もこのフラグでスキップし、LastTab 分岐が確定した
    /// layout.json の削除 / 保持を上書きしない）
    quitting: bool,
    /// ペインを保持する tmux バックエンドセッション名（persist 有効時のみ登録される）
    backend_sessions: HashMap<PaneId, String>,
    /// orphan 復元（#191）で旧 pane ID から新 pane ID へのマッピング。
    /// 既存の claude CLI プロセスが旧 TAKO_PANE_ID で MCP を呼んだとき、
    /// dispatch で resolve 失敗 → このマップで新 pane ID に解決する（#210）
    stale_pane_map: HashMap<PaneId, PaneId>,
    /// PC 再起動で tmux セッション自体が消えたときに resume する Claude session ID。
    /// `claude agents --json` と PID 祖先照合が成功した結果だけを保持する
    claude_resume_sessions: HashMap<PaneId, String>,
    /// バックエンドセッション内の window 一覧（tmux ポーリングで更新。2+ window のみ保持）
    backend_windows: HashMap<PaneId, Vec<tako_core::TmuxWindow>>,
    /// tmux window のキャプチャテキスト（ホバープレビュー用。ポーリングで非アクティブ window を取得）
    window_captures: HashMap<(PaneId, u32), Vec<String>>,
    /// 直近に保存したレイアウトの JSON（変化したときだけ書き込むための比較用）
    last_saved_layout: Option<String>,
    /// 起動時のレイアウト復元結果（人間可読 1 行。Issue #30 の診断用。
    /// `tako persist` / MCP `tako_persist` の `last_restore` として公開する）
    restore_report: Option<String>,
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
    /// ペインヘッダ / タブの右クリックメニュー（#185）
    pane_context_menu: Option<PaneContextMenu>,
    /// ファイルツリーのインライン編集
    inline_edit: Option<InlineEdit>,
    /// D&D 中のペイロード種別（FR-2.16.10 / FR-3.11）。on_drag 開始でセット、
    /// drop / mouse-up でクリア。gpui の active_drag は型を公開しないため自前で追跡し、
    /// ドロップ先オーバーレイの生成判定 + ラベル出し分けに使う
    drag_kind: Option<DragKind>,
    /// ドラッグ中のドロップ先（ペイン, 挿入位置）。挿入プレビュー表示の状態
    drop_target: Option<(PaneId, DropZone)>,
    /// タブバーへのペイン D&D: ドロップ先タブ（Some(id) = 既存タブへ合流、None = 新タブ化）
    tab_drop_target: Option<Option<TabId>>,
    /// タブ D&D 並べ替え中の挿入位置インジケータ（#308）。
    /// Some(tab_id) = そのタブの**左**にインジケータを表示、None = 末尾
    tab_reorder_indicator: Option<Option<TabId>>,
    /// git パネルのデータ（FR-3.6。cwd 連動で 2 秒ポーリング更新）
    git_data: Option<GitPanelData>,
    /// サイドバー用の軽量 git サマリ（#217。ブランチチップ + 変更フッター）
    sidebar_git: Option<SidebarGitSummary>,
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
    /// 子ワーカードロップダウンを開いている master ペイン（#217。「N workers ▾」）
    workers_menu_open: Option<PaneId>,
    /// Attention トースト（#217。失敗の即時通知。右下に積む）
    toasts: Vec<AttentionToast>,
    /// トースト検知用: 前回スナップショットで Failed だったペイン（#217）
    known_failed: std::collections::HashSet<PaneId>,
    /// ⌘K コマンドパレット（#217。None = 閉じている）
    command_palette: Option<CommandPalette>,
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
    /// ステータスバーの利用制限表示で選択中のサービス（Issue #321。settings.json 永続化）
    limit_service: tako_core::LimitService,
    /// ステータスバーの利用制限サービス切替ドロップダウンが開いているか（Issue #321）
    limit_service_menu_open: bool,
    /// usage トークン推移の履歴（#217 スパークライン。最大 5 点、変化時のみ追記）
    usage_history: std::collections::VecDeque<f32>,
    /// 端末イベントの再描画デバウンス: 最後に notify した時刻
    last_term_notify: std::time::Instant,
    /// 端末イベントの再描画デバウンス: 遅延 notify のタイマーが稼働中か
    term_notify_pending: bool,
    /// 動画フレームの描画キャッシュ（frame_gen で世代管理: 新フレーム準備完了まで前フレームを表示）
    video_frame_cache: HashMap<PaneId, (u64, std::sync::Arc<gpui::RenderImage>)>,
    /// 動画の旧フレーム。次の render 冒頭で GPU sprite atlas から解放する（Issue #258）。
    pending_video_frame_evictions: Vec<std::sync::Arc<gpui::RenderImage>>,
    /// PDF / 画像 / 動画サムネの描画用 gpui::Image キャッシュ（Issue #168）。
    /// `Image::from_bytes` は id 生成で全バイトのハッシュ計算を行うため、毎フレーム
    /// 生成すると PDF 全ページの PNG コピー + ハッシュだけで 1 フレーム 100ms 級になる
    /// （71 ページ PDF の実測 p50 96ms/frame）。path 不変の間は Arc を再利用する
    preview_image_cache: HashMap<PaneId, PreviewImageCache>,
    /// デコード済み画像の CPU バイト予算を管理するプロセス全体 LRU（Issue #258）。
    preview_image_lru: tako_core::ByteLru<PreviewImageCacheEntryKey>,
    /// LRU / close で外した GPUI asset。次の render 冒頭で atlas と一緒に解放する。
    pending_preview_image_evictions: Vec<std::sync::Arc<gpui::Image>>,
    /// チェンジログビューのデータ（Issue #338。pane ごと）
    preview_changelogs: HashMap<PaneId, preview::ChangelogData>,
    /// PDF・画像プレビューのズーム / パン / ページ状態（#234。core モデル）。
    preview_views: HashMap<PaneId, tako_core::PreviewViewState>,
    /// PDF・画像プレビューの 2 軸スクロールとページ移動を共有する GPUI handle。
    preview_scroll_handles: HashMap<PaneId, gpui::ScrollHandle>,
    /// シークバー要素の実測 bounds（paint 時に canvas で記録）
    video_seek_bar_bounds: HashMap<PaneId, Bounds<Pixels>>,
    /// シークバーのドラッグ中フラグ（ペイン ID。ドラッグ中はマウス移動でシーク位置を追従）
    video_seek_dragging: Option<PaneId>,
    /// シークバーのホバー時刻（ペイン ID、秒数、x 座標）
    video_seek_hover: Option<(PaneId, f64, f32)>,
    /// プレビューペインのテキスト選択
    preview_selections: HashMap<PaneId, PreviewSelection>,
    /// プレビューで選択操作中のペイン
    preview_selecting: Option<PaneId>,
    /// プレビューの行ごとの bounds（paint 時に canvas で記録。選択のヒット判定用）
    preview_line_bounds: HashMap<PaneId, Vec<Bounds<Pixels>>>,
    /// PDF プレビューの文字ごとの bounds（paint 時に canvas で記録。選択のヒット判定用）
    preview_pdf_char_bounds: HashMap<PaneId, Vec<Vec<Bounds<Pixels>>>>,
    /// PDF 選択の最前面 canvas が直近フレームで発行した矩形数。
    /// selftest が座標計算だけでなく paint 経路まで到達したことを検証する。
    preview_pdf_highlight_paint_count: HashMap<PaneId, usize>,
    /// PDF リンクのホバー状態（#271）。⌘ 押下中のみ有効。
    preview_pdf_hovered_link: Option<(PaneId, usize)>,
    /// PDF ページ画像のスクリーン座標 bounds（canvas paint 時に直接記録。#315）。
    /// estimate 不要で zoom / scroll 後もヒットテストが正しい。
    preview_pdf_page_image_bounds: HashMap<PaneId, HashMap<usize, Bounds<Pixels>>>,
    /// コード / Markdown の行ごとの GPUI 実描画レイアウト。
    /// 描画と同じ shaping 結果で座標を UTF-8 byte index へ逆写像する。
    preview_text_layouts: HashMap<PaneId, Vec<Option<TextLayout>>>,
    /// プレビューの行ごとのプレーンテキスト（選択テキスト抽出用）
    preview_line_texts: HashMap<PaneId, Vec<String>>,
    /// ネイティブ Web ビュー（FR-3.8 / #155）。表示中 + dock 退避中の全ページを
    /// ペインから独立に保持する（ペインを閉じてもページ = wry WebView が生きる）
    webviews: Vec<webview::WebViewEntry>,
    /// Web ビューの dock 管理用 ID 採番
    webview_next_id: u64,
    /// 今フレームで描画された Web ビュー（render 末尾の可視性同期で使う）
    webview_marks: std::collections::HashSet<webview::WebViewId>,
    /// Web ビュー dock パネルの開閉（ステータスバーの Web ボタン）
    webview_dock_open: bool,
    /// Web ビュー dock の URL 入力欄（#207）
    webview_dock_url_input: String,
    /// URL 入力欄のカーソル位置（バイト）
    webview_dock_url_cursor: usize,
    /// URL 入力欄がフォーカスされているか（dock は開いていてもターミナルへ入力できる）
    webview_dock_url_focused: bool,
    /// wry の親にする GPUI ウィンドウの生ハンドル（初回 render で採取）
    window_raw_handle: Option<webview::WindowHandleBox>,
    /// 起動復元で開き直す Web ビュー（ペイン対応, URL）。ウィンドウハンドルが
    /// 要るため初回 render で消費する
    pending_webview_restore: Vec<(Option<u64>, String)>,
    /// アプリ内自動更新の状態
    update_state: update_checker::UpdateState,
    /// グリフ advance がセル幅（半角 1 セル）と一致するかのキャッシュ（Issue #64）。
    /// テーマフォントに無いグリフ（⏺ ⎿ 等）はフォールバックフォントで描画され
    /// advance がセル幅とずれるため、描画グループ化から除外する判定に使う
    glyph_snap_cache: std::cell::RefCell<HashMap<char, bool>>,
    /// グリフ advance 実測用のテキストシステム（new で App から取得して保持）
    text_system: std::sync::Arc<gpui::TextSystem>,
    /// cmd+ホバー中のリンク検出結果キャッシュ（ペインごと）
    pane_links: HashMap<PaneId, Vec<tako_core::DetectedLink>>,
    /// cmd+ホバーでヒットしているリンクのターゲット（視覚フィードバック用）
    hovered_link: Option<HoveredLink>,
    /// × ボタン close の確認ダイアログ（Issue #172。config.yaml で永続化）
    confirm_close: bool,
    /// 確認ダイアログ表示中の対象（None = ダイアログ非表示）
    pending_close_confirm: Option<CloseConfirmTarget>,
    /// スリープ防止のアサーションが現在保持中か（ステータスバー表示用。ポーリングで更新）
    sleep_guard_active: bool,
    /// 蓋が閉じているか（#218 ステータスバー表示用）
    lid_closed: bool,
    /// pmset disablesleep が有効か（#218 ステータスバー表示用）
    lid_sleep_disabled: bool,
    /// thermal 警告中か（#218 ステータスバー表示用）
    thermal_warning: bool,
    /// 起動時に orphan 自動復帰した tmux セッション数（Issue #191。診断用）
    recovered_count: usize,
    /// ペインの平文ログ管理（Issue #112 B）
    pane_logs: std::sync::Arc<std::sync::Mutex<tako_core::pane_log::PaneLogManager>>,
    /// 自動保存が必要なプレビューペイン（デバウンスタイマーで消費。#195）
    autosave_pending: std::collections::HashSet<PaneId>,
    /// タブバーの横スクロール（Issue #208。GPUI ScrollHandle で位置を制御）
    tab_scroll_handle: gpui::ScrollHandle,
    /// 前回 render 時のアクティブタブ ID（タブ切替時のみ自動スクロール発火用。Issue #208）
    last_active_tab: Option<TabId>,
    /// タイトルバー（タブバー領域）のドラッグでウインドウ移動中か（#312）
    titlebar_dragging: bool,
}

/// × ボタン close の確認ダイアログ対象（Issue #172）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseConfirmTarget {
    Pane(PaneId),
    Tab(TabId),
}

/// tmux バックエンドペインのペインログ取り込みジョブ（Issue #112 B）。
/// UI スレッドで文脈だけ集め、probe / capture（サブプロセス実行）は background で行う
struct PaneLogJob {
    pane: PaneId,
    session: String,
    meta: tako_core::pane_log::PaneLogMeta,
    last_history: usize,
    last_bytes: u64,
}

/// ペインログのクローズフラッシュ素材（close 前に UI で採取する）
struct PaneLogCloseData {
    meta: tako_core::pane_log::PaneLogMeta,
    visible: Vec<String>,
    /// 直接ペインの最終履歴増分（取り込み行, delta, 現在履歴行数）
    catch_up: Option<(Vec<String>, usize, usize)>,
}

/// バックエンドペインのペインログ取り込み本体（Issue #112 B。background で実行）。
/// probe → 増分 capture → manager へ反映。セッション消滅・tmux 不在はスキップする
fn process_pane_log_jobs(
    manager: &std::sync::Arc<std::sync::Mutex<tako_core::pane_log::PaneLogManager>>,
    socket: &str,
    jobs: Vec<PaneLogJob>,
) {
    use tako_core::pane_log::{ChunkKind, PaneObservation, CAPTURE_CHUNK};
    for job in jobs {
        let Some(probe) = tako_core::tmux::pane_log_probe(Some(socket), &job.session) else {
            continue;
        };
        let chunk = if probe.history < job.last_history {
            ChunkKind::None
        } else {
            let delta = probe.history - job.last_history;
            if delta > 0 {
                let take = delta.min(CAPTURE_CHUNK);
                match tako_core::tmux::capture_history_plain(Some(socket), &job.session, take) {
                    Some(lines) => ChunkKind::Counted { lines, delta },
                    None => continue,
                }
            } else if probe.history >= probe.limit
                && !probe.alternate
                && probe.bytes != job.last_bytes
            {
                // 履歴が history-limit で飽和するとカウンタが増えない。バイト数の変化を
                // 合図に末尾チャンクを取り、取り込み済み tail と照合して新規行だけ追記する
                match tako_core::tmux::capture_history_plain(
                    Some(socket),
                    &job.session,
                    CAPTURE_CHUNK,
                ) {
                    Some(captured) => ChunkKind::Overlap { captured },
                    None => continue,
                }
            } else {
                ChunkKind::None
            }
        };
        let mut mgr = manager
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mgr.apply(
            job.pane.as_u64(),
            &job.meta,
            PaneObservation {
                history: probe.history,
                history_limit: probe.limit,
                bytes: probe.bytes,
                alt_screen: probe.alternate,
                chunk,
            },
        );
    }
}

/// cmd+ホバーで検出されたリンク情報
#[derive(Debug, Clone)]
struct HoveredLink {
    pane: PaneId,
    target: String,
    kind: tako_core::LinkKind,
    spans: Vec<(usize, usize, usize)>,
}

impl HoveredLink {
    fn contains(&self, pane: PaneId, row: usize, col: usize) -> bool {
        self.pane == pane
            && self
                .spans
                .iter()
                .any(|&(r, start, end)| r == row && start <= col && col < end)
    }
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

/// サイドバー用の軽量 git サマリ（#217。ブランチチップ + 変更フッター）
#[derive(Debug, Clone, PartialEq, Eq)]
struct SidebarGitSummary {
    branch: String,
    modified: usize,
    added_lines: usize,
    removed_lines: usize,
}

/// git パネルのアコーディオン折りたたみ状態
#[derive(Debug, Clone, Default)]
struct GitCollapsed {
    branches: bool,
    changes: bool,
    commits: bool,
    diff: bool,
}

/// ミラースクロールの実体解決の起点（#181）。
/// Backend はスクロール開始時にネスト tmux を辿って解決する（background executor）、
/// Fixed は解決済み（TmuxOpen ビュー）
enum MirrorSource {
    Backend(String),
    Fixed(tako_core::scroll::ScrollTarget),
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

/// terminal_screen_lines の 1 文字ぶんの描画情報（#39 / #64）
#[derive(Debug, Clone)]
struct CharInfo {
    ch: char,
    /// 占有セル数（全角 = 2、半角 = 1、結合文字等 = 0）
    char_cols: usize,
    /// line.runs のインデックス（範囲外 = フォールバックスタイル）
    run_idx: usize,
    bg: Option<tako_core::theme::Rgb>,
    /// グリフ advance が半角セル幅と一致するか（char_cols == 1 のときのみ意味を持つ）
    snaps: bool,
}

/// 描画 div 1 つぶんのチャンク（CharInfo 列の半開区間）
#[derive(Debug, Clone, PartialEq)]
struct RenderChunk {
    start: usize,
    end: usize,
    /// 合計セル数
    cols: usize,
    run_idx: usize,
    bg: Option<tako_core::theme::Rgb>,
}

/// 同スタイル・セル幅整合の連続半角文字を 1 チャンクへまとめ、全角文字と
/// セル幅不一致グリフ（snaps == false）は単独チャンクに分離する。
/// グループ化は #39（描画要素数の削減）、不一致グリフの分離は #64
/// （フォールバックフォントの advance ずれがグループ内で累積し後続文字を
/// 押し出すのを、セル幅固定の個別 div で遮断する）。
/// ゼロ幅文字（結合文字）は直前のグループに含める（分離するとベース文字と合成されない）
fn chunk_line_chars(infos: &[CharInfo]) -> Vec<RenderChunk> {
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < infos.len() {
        let info = &infos[i];
        let start = i;
        let mut cols = info.char_cols;
        let solo = info.char_cols > 1 || (info.char_cols == 1 && !info.snaps);
        i += 1;
        if !solo {
            while i < infos.len() {
                let next = &infos[i];
                if next.char_cols > 1
                    || (next.char_cols == 1 && !next.snaps)
                    || next.run_idx != info.run_idx
                    || next.bg != info.bg
                {
                    break;
                }
                cols += next.char_cols;
                i += 1;
            }
        }
        chunks.push(RenderChunk {
            start,
            end: i,
            cols,
            run_idx: info.run_idx,
            bg: info.bg,
        });
    }
    chunks
}

/// 描画チャンク内でリンクのセル範囲に重なる UTF-8 バイト範囲を返す。
/// StyledText のハイライトをリンク部分だけへ限定し、同じ ANSI style run の行全体へ
/// 下線・背景色が広がるのを防ぐ。
fn link_byte_range_in_chunk(
    infos: &[CharInfo],
    cell_cols: &[usize],
    chunk: &RenderChunk,
    link_start: usize,
    link_end: usize,
) -> Option<Range<usize>> {
    let mut byte_offset = 0;
    let mut start = None;
    let mut end = 0;

    for (index, info) in infos.iter().enumerate().take(chunk.end).skip(chunk.start) {
        let cell_start = cell_cols.get(index).copied().unwrap_or(index);
        let cell_end = cell_start + info.char_cols;
        let next_byte = byte_offset + info.ch.len_utf8();
        let overlaps = if info.char_cols == 0 {
            cell_start >= link_start && cell_start < link_end
        } else {
            cell_end > link_start && cell_start < link_end
        };
        if overlaps {
            start.get_or_insert(byte_offset);
            end = next_byte;
        } else if start.is_some() {
            break;
        }
        byte_offset = next_byte;
    }

    start.map(|start| start..end)
}

/// プレビューペインのテキスト選択状態
#[derive(Debug, Clone)]
struct PreviewSelection {
    anchor: (usize, usize),
    head: (usize, usize),
}

/// 最新の PDF 再ラスタライズ要求。path と量子化済みキーが一致した結果だけを採用する。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingPdfRaster {
    path: std::path::PathBuf,
    key: preview::PdfRasterKey,
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

/// どのタブにも表示されていない tmux セッション 1 件分（FR-2.16.8 / #183）。
/// `orphan_backend` = tako から起動されたが対応ペインを失った残骸（kill 漏れ?）、
/// false = tako 管理外（ユーザーが直接立てた等）
#[derive(Debug, Clone)]
struct UnlistedTmuxSession {
    name: String,
    socket: Option<String>,
    orphan_backend: bool,
    attached: bool,
    /// (window index, 表示ラベル)
    windows: Vec<(u32, String)>,
    /// ロール（TAKO_ORCHESTRATOR_ROLE。セッション名から推定）
    role: String,
    /// 中で走っているプロセス名
    process: String,
    /// セッションの cwd
    cwd: String,
    /// 最終アクティビティからの経過（人間可読）
    last_activity_age: String,
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
    is_pinned_root: bool,
    position: Point<Pixels>,
}

/// ペインヘッダ / タブの右クリックメニュー（#185）
struct PaneContextMenu {
    pane: PaneId,
    kind: PaneContextKind,
    position: Point<Pixels>,
}

#[derive(Clone, Copy)]
enum PaneContextKind {
    Terminal,
    Preview,
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
    /// Finder など外部アプリからのファイルドロップ（ExternalPaths 経由）。
    /// ターミナルペインではパス入力、それ以外ではファイルを開く
    ExternalFile,
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
        // 多重インスタンスガード（Issue #113）: 別の生きたインスタンスがプライマリである間は
        // **セカンダリモード**で起動する（復元しない・layout.json に触らない・tmux バックエンドに
        // 乗らない・固定ソケットを乗っ取らない）。ガード無しだと、後発の復元 spawn が
        // `new-session -A -D` でプライマリの全クライアントを強奪 → プライマリ側の Exited 連鎖の
        // 途中状態が layout.json を上書き → 次回起動の orphan cleanup が protected から漏れた
        // 実行中セッションを kill する三段連鎖が起きる（2026-07-08 実機: 19→13 ペイン消失）。
        // - プロセス内: NewWindow で 2 つ目以降の TakoApp（最初の 1 つだけがプライマリ）
        // - プロセス間: control.json の主がまだ生きた tako-app（SIGKILL 残骸は pid 死亡で除外）
        // TAKO_FORCE_PRIMARY=1 は検証・緊急脱出用の明示オーバーライド（多重復元の保護も切れる）
        static PRIMARY_CLAIMED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        let in_process_secondary = PRIMARY_CLAIMED.swap(true, std::sync::atomic::Ordering::SeqCst);
        let secondary_reason: Option<String> = if std::env::var_os("TAKO_SELF_TEST").is_some()
            || std::env::var_os("TAKO_FORCE_PRIMARY").is_some()
        {
            None
        } else if in_process_secondary {
            Some("同一プロセスの先行ウィンドウがプライマリ".into())
        } else {
            // discovery ベースの判定は TAKO_DISCOVERY_DIR の隔離や control.json の
            // 消失で盲目になるため、守るべき資源そのもの（復元対象 tmux セッションの
            // クライアント）を見る第二ガードを重ねる（#177）
            tako_control::discovery::live_primary_pid()
                .map(|pid| format!("別の tako プロセス（pid {pid}）が稼働中"))
                .or_else(foreign_client_guard)
        };
        let secondary = secondary_reason.is_some();

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
            match IpcServer::start_with(control_tx.clone(), token.clone(), secondary) {
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
        // から `tako` CLI が繋ぎ直せるようにする（CLI 側のフォールバック先）。
        // セカンダリモードは current ポインタ（control.json）を奪わず instances/ のみに書く
        // （プライマリへの既存 CLI / MCP 接続を壊さない。Issue #113）
        if let (Some(ipc), Some(token)) = (&ipc, &token) {
            let info = tako_control::discovery::ControlInfo {
                version: 1,
                pid: std::process::id(),
                socket: ipc.endpoint().to_string(),
                token: token.clone(),
                mcp_url: mcp.as_ref().map(|m| m.url().to_string()),
            };
            let written = if secondary {
                tako_control::discovery::write_instance_only(&info)
            } else {
                tako_control::discovery::write(&info)
            };
            if let Err(e) = written {
                eprintln!("warning: 接続情報ファイルを書き出せない（再起動後の外部接続は環境変数頼みになる）: {e}");
            }
        }

        // 再起動からの復元（Phase 5.5 / FR-5）。persist 有効ならレイアウトファイルから
        // 同じ ID・同じ構成でワークスペースを再現する。tmux があれば各ペインは下の
        // spawn ループでバックエンドセッションへ再 attach（実行中プロセスごと復元）、
        // tmux 不在なら保存 cwd で新しいシェルを開く**構造のみ復元**に劣化する
        // （Issue #30: tmux の有無でレイアウト永続化そのものを無効化しない）。
        // セカンダリモードは persist 一式（復元・保存・バックエンド）を無効化する（Issue #113）
        let tmux_persist = initial_tmux_persist() && !secondary;
        let tmux_available = tako_core::tmux_backend::available();
        if tmux_persist && tmux_available {
            // 生き残っている既存サーバーへ最新 conf を再適用する（conf は
            // サーバー起動時にしか読まれないため、バージョン更新の設定変更が
            // ここで同期されないと永久に届かない）
            tako_core::tmux_backend::sync_conf(&tako_core::tmux_backend::socket_name());
        }
        let mut restored: Vec<tako_control::layout::RestoredPane> = Vec::new();
        let mut collapsed_tmux_tabs: std::collections::HashSet<TabId> =
            std::collections::HashSet::new();
        // Web ビュー dock の退避分（#155。表示分は RestoredPane.webview で運ばれる）
        let mut webview_dock_restore: Vec<String> = Vec::new();
        let (workspace, restore_report) = if let Some(reason) = &secondary_reason {
            let msg = format!(
                "復元スキップ: {reason}のためセカンダリモードで起動\
                 （このウィンドウのタブは永続化されず、既存側の復元・保存を妨げない）"
            );
            persist_diag(&msg);
            (Workspace::new("1", Pane::new(PaneOrigin::User)), msg)
        } else if tmux_persist {
            let loaded = tako_control::layout::try_load().and_then(|file| {
                // 折りたたみ状態（FR-2.16.14）を控えてから復元する。タブ ID は
                // Phase 5.5 で同一値に復元されるので再起動後も対応が保たれる
                collapsed_tmux_tabs = file
                    .collapsed
                    .iter()
                    .map(|id| TabId::from_raw(*id))
                    .collect();
                webview_dock_restore = file.webview_dock.clone();
                tako_control::layout::restore(&file)
            });
            match loaded {
                Ok((ws, panes)) => {
                    restored = panes;
                    let msg = format!(
                        "復元成功: {} タブ / {} ペイン（{}）",
                        ws.tabs().len(),
                        restored.len(),
                        if tmux_available {
                            "tmux あり: 実行中プロセスごと再 attach"
                        } else {
                            "tmux 不在: タブ構成のみ・新シェルで開き直し"
                        }
                    );
                    persist_diag(&msg);
                    (ws, msg)
                }
                Err(e) => {
                    let msg = if e == tako_control::layout::LayoutError::NotFound {
                        // 初回起動・明示クローズ後の正常系。理由だけ記録する
                        format!("復元なし: {e}")
                    } else {
                        // 破損・不整合ファイルは .corrupt へ退避して原因調査に残す
                        // （放置すると次の定期保存で黙って上書きされるため）
                        let stashed = tako_control::layout::layout_path()
                            .map(|p| std::fs::rename(&p, p.with_extension("json.corrupt")).is_ok())
                            .unwrap_or(false);
                        format!(
                            "復元失敗: {e}{}",
                            if stashed {
                                "（layout.json.corrupt へ退避）"
                            } else {
                                ""
                            }
                        )
                    };
                    persist_diag(&msg);
                    (Workspace::new("1", Pane::new(PaneOrigin::User)), msg)
                }
            }
        } else {
            let msg = "復元なし: persist 無効（設定または環境変数で OFF）".to_string();
            persist_diag(&msg);
            (Workspace::new("1", Pane::new(PaneOrigin::User)), msg)
        };
        let restore_report = Some(restore_report);

        let preview_reload_enabled = initial_preview_reload();
        let (preview_file_watcher, preview_watch_rx) =
            match preview_watch::PreviewFileWatcher::new() {
                Ok((watcher, rx)) => (Some(watcher), Some(rx)),
                Err(_) => {
                    eprintln!("warning: プレビューのファイル監視を開始できない");
                    (None, None)
                }
            };

        let mut app = Self {
            // ルートペイン（復元時は全ペイン）は下の spawn_session でセッションを張る
            workspace,
            terminals: HashMap::new(),
            // テーマは settings.json の設定値から復元（Issue #217。既定ダーク）
            theme: Theme::for_mode(tako_control::settings::load().theme_mode()),
            focus_handle: cx.focus_handle(),
            cell_size: None,
            pane_font_sizes: HashMap::new(),
            pane_cell_sizes: HashMap::new(),
            selecting: None,
            drag_scroll: None,
            pane_text_areas: Vec::new(),
            ipc,
            mcp,
            token,
            pending_attach: Vec::new(),
            pending_writes: Vec::new(),
            alt_screen_writes: Vec::new(),
            prompt_flows: Vec::new(),
            pending_highlights: Vec::new(),
            pending_preview_loads: Vec::new(),
            preview_device_scale: 1.0,
            pending_pdf_rasters: HashMap::new(),
            active_pdf_rasters: std::collections::HashSet::new(),
            ime: None,
            dragging_border: None,
            dragging_scrollbar: None,
            hovered_scrollbar: None,
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
            pane_context_menu: None,
            inline_edit: None,
            sidebar_width: {
                let w = tako_control::settings::load().sidebar_width as f32;
                w.clamp(SIDEBAR_MIN_WIDTH, 600.0)
            },
            dragging_sidebar: false,
            filetree: filetree::FileTree::default(),
            previews: HashMap::new(),
            preview_navigation_panel: None,
            preview_reload: tako_core::PreviewReloadState::new(
                preview_reload_enabled && preview_file_watcher.is_some(),
            ),
            preview_file_watcher,
            pending_preview_reloads: HashMap::new(),
            active_preview_reloads: std::collections::HashSet::new(),
            active_preview_reload_jobs: std::collections::HashSet::new(),
            preview_reload_generations: HashMap::new(),
            next_preview_reload_generation: 0,
            preview_reload_apply_count: 0,
            preview_edits: HashMap::new(),
            autorename: autorename::AutoRenamer::new(initial_auto_rename()),
            port_detect: initial_port_detect(),
            port_suggestions: Vec::new(),
            dismissed_ports: std::collections::HashSet::new(),
            tmux_persist,
            secondary,
            quitting: false,
            backend_sessions: HashMap::new(),
            stale_pane_map: HashMap::new(),
            claude_resume_sessions: HashMap::new(),
            backend_windows: HashMap::new(),
            window_captures: HashMap::new(),
            last_saved_layout: None,
            restore_report,
            window_frame: None,
            drag_kind: None,
            drop_target: None,
            tab_drop_target: None,
            tab_reorder_indicator: None,
            git_data: None,
            sidebar_git: None,
            git_selected_commit: None,
            git_collapsed: GitCollapsed::default(),
            drawer_visible: false,
            drawer_height: DRAWER_DEFAULT_HEIGHT,
            bg_pending_kill: None,
            hover_preview: None,
            workers_menu_open: None,
            toasts: Vec::new(),
            known_failed: std::collections::HashSet::new(),
            command_palette: None,
            pinned_previews: Vec::new(),
            dragging_pin: None,
            agent_metrics: AgentMetrics::default(),
            limit_service: tako_control::settings::load().limit_service(),
            limit_service_menu_open: false,
            usage_history: std::collections::VecDeque::new(),
            last_term_notify: std::time::Instant::now(),
            term_notify_pending: false,
            video_players: HashMap::new(),
            video_ticker: false,
            video_frame_cache: HashMap::new(),
            pending_video_frame_evictions: Vec::new(),
            preview_image_cache: HashMap::new(),
            preview_image_lru: tako_core::ByteLru::new(initial_preview_cache_budget()),
            pending_preview_image_evictions: Vec::new(),
            preview_changelogs: HashMap::new(),
            preview_views: HashMap::new(),
            preview_scroll_handles: HashMap::new(),
            video_seek_bar_bounds: HashMap::new(),
            video_seek_dragging: None,
            video_seek_hover: None,
            preview_selections: HashMap::new(),
            preview_selecting: None,
            preview_line_bounds: HashMap::new(),
            preview_pdf_char_bounds: HashMap::new(),
            preview_pdf_highlight_paint_count: HashMap::new(),
            preview_pdf_hovered_link: None,
            preview_pdf_page_image_bounds: HashMap::new(),
            preview_text_layouts: HashMap::new(),
            preview_line_texts: HashMap::new(),
            webviews: Vec::new(),
            webview_next_id: 1,
            webview_marks: std::collections::HashSet::new(),
            webview_dock_open: false,
            webview_dock_url_input: String::new(),
            webview_dock_url_cursor: 0,
            webview_dock_url_focused: false,
            window_raw_handle: None,
            pending_webview_restore: Vec::new(),
            update_state: update_checker::UpdateState::Idle,
            glyph_snap_cache: std::cell::RefCell::new(HashMap::new()),
            text_system: cx.text_system().clone(),
            pane_links: HashMap::new(),
            hovered_link: None,
            confirm_close: tako_control::setup::confirm_close_enabled(),
            pending_close_confirm: None,
            sleep_guard_active: false,
            lid_closed: false,
            lid_sleep_disabled: false,
            thermal_warning: false,
            recovered_count: 0,
            pane_logs: std::sync::Arc::new(std::sync::Mutex::new(
                tako_core::pane_log::PaneLogManager::new(
                    tako_core::pane_log::log_dir()
                        .unwrap_or_else(|| std::env::temp_dir().join("tako-pane-logs")),
                    tako_control::settings::load().pane_log_config(),
                ),
            )),
            autosave_pending: std::collections::HashSet::new(),
            tab_scroll_handle: gpui::ScrollHandle::new(),
            last_active_tab: None,
            titlebar_dragging: false,
        };
        // App Nap 無効化 + 初回スリープ防止更新（Issue #173）
        // 蓋閉じ防止の残留チェック（#218: 前回クラッシュ時の disablesleep=1 を自動復帰）
        tako_control::sleep_guard::disable_app_nap();
        tako_control::sleep_guard::check_disablesleep_residual();
        app.update_sleep_guard();

        // 終了処理（layout 保存 + 接続情報の後片付け）はアプリ終了フックで一元化する
        // （#103）。Cmd-Q（グローバル Quit アクション）・メニュー・Dock 右クリック終了・
        // OS シャットダウンのどの経路でも走る（従来はルート div の Quit ハンドラ限定で、
        // Dock 終了では操作ごと保存に救われているだけだった）。
        // 「全ペイン終了」経路（quitting=true）は layout.json の削除 / 保持を
        // close_pane 側で確定済みのため、ここでは触らない（#30 / #113 の挙動を維持）
        cx.on_app_quit(|this: &mut TakoApp, _cx| {
            if !this.quitting {
                // ペインログ（Issue #112 B）: バックエンドの無い直接ペインはアプリと共に
                // プロセスが死ぬため、可視画面をフラッシュして書き残す。バックエンドペインは
                // tmux 側で生き続け、再起動後に logged_history から差分取り込みするので触らない
                let direct_panes: Vec<PaneId> = this
                    .terminals
                    .keys()
                    .filter(|p| !this.backend_sessions.contains_key(p))
                    .copied()
                    .collect();
                for pane in direct_panes {
                    if let Some(data) = this.pane_log_close_data(pane) {
                        this.apply_pane_log_close(pane, data, CloseReason::Exited);
                    }
                }
                // 蓋閉じ防止の解除（#218: 正常終了時に disablesleep を 0 に戻す）
                tako_control::sleep_guard::cleanup_on_exit();
                // 終了直前の構成を保存してから抜ける（Phase 5.5。セッションは残る = 永続化）
                this.save_layout();
                // persist ON（セッション生存）なら接続情報を残す: ソケットパス・トークンが
                // 再起動後も同一のため、既存クライアントがそのまま再接続できる。
                // persist OFF なら旧来通り片付け（死んだ接続先を CLI の候補に残さない）
                if !this.tmux_persist {
                    tako_control::discovery::cleanup(std::process::id());
                }
            }
            // セルフテスト最終項目（フォーカス喪失状態の cmd-q。#103）の成功マーカー。
            // ここに到達した = Quit がフォーカス非依存で発火し quit 経路に入った証拠。
            // 全 check 通過後にだけ cmd-q が送られるため、これが総合 OK マーカーを兼ねる
            if std::env::var_os("TAKO_SELF_TEST").is_some() {
                println!("TAKO_APP_SELF_TEST_OK");
            }
            async {}
        })
        .detach();
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
            let mut reattached = 0usize;
            let mut resumed_claude = 0usize;
            let mut fresh_shells = 0usize;
            let mut restored_previews = 0usize;
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
                    // #168 / #232: Markdown 目次・PDF・動画は復元時も background 読み込み
                    let state = if matches!(
                        mode,
                        preview::PreviewMode::Markdown
                            | preview::PreviewMode::Pdf
                            | preview::PreviewMode::Video
                    ) {
                        app.pending_preview_loads
                            .push((pane, path.to_path_buf(), mode));
                        preview::PreviewState::loading(path, mode)
                    } else {
                        let (state, raw) = preview::load_fast(path, mode);
                        if let Some(text) = raw {
                            app.pending_highlights
                                .push((pane, path.to_path_buf(), text));
                        }
                        state
                    };
                    app.previews.insert(pane, state);
                    restored_previews += 1;
                    continue;
                }
                // Web ビューペイン（FR-3.8 / #155）は URL を開き直すだけ（PTY は起動しない）。
                // wry の生成にはウィンドウハンドルが要るため初回 render まで遅延する
                if let Some(url) = &r.webview {
                    app.pending_webview_restore
                        .push((Some(r.pane), url.clone()));
                    continue;
                }
                let backend_alive = r.session.as_ref().is_some_and(|name| {
                    tmux_available
                        && tako_core::tmux::has_session(
                            Some(&tako_core::tmux_backend::socket_name()),
                            name,
                        )
                });
                if let Some(name) = &r.session {
                    app.backend_sessions.insert(pane, name.clone());
                }
                // ペインログ（Issue #112 B）: 前回の取り込み位置を復元し、tako 停止中に
                // tmux 側へ積もった出力を次回 tick の差分として取り込む
                if let Some(history) = r.logged_history {
                    let meta = app.pane_log_meta(pane);
                    app.pane_logs_lock()
                        .seed_history(pane.as_u64(), &meta, history as usize);
                }
                let transcript_exists = r
                    .claude_session_id
                    .as_deref()
                    .is_some_and(|id| tako_control::transcript::find_transcript(id).is_some());
                let resume_command = claude_resume_command(
                    backend_alive,
                    r.claude_session_id.as_deref(),
                    transcript_exists,
                );
                if let Some(session_id) = &r.claude_session_id {
                    if tako_control::transcript::is_valid_session_id(session_id) {
                        app.claude_resume_sessions.insert(pane, session_id.clone());
                    }
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
                    continue;
                }
                if backend_alive {
                    reattached += 1;
                } else if let Some(command) = resume_command {
                    // tmux サーバーごと消える PC 再起動では新しいログインシェルを起動し、
                    // 保存済みの会話だけを明示 resume する。入力を PTY にキューすることで、
                    // Claude 終了後は元のシェルへ戻れる（明示コマンド spawn だとペインも終了する）。
                    if let Some(session) = app.terminals.get(&pane) {
                        session.write(command);
                        resumed_claude += 1;
                    }
                } else {
                    fresh_shells += 1;
                }
            }
            if app.terminals.is_empty()
                && app.previews.is_empty()
                && app.pending_webview_restore.is_empty()
            {
                eprintln!("fatal: 復元したペインを 1 つも起動できない");
                std::process::exit(1);
            }
            let report = format!(
                "復元成功: {} タブ / {} ペイン（tmux 再 attach {} / Claude resume {} / 新規シェル {} / プレビュー {}）",
                app.workspace.tabs().len(),
                restored.len(),
                reattached,
                resumed_claude,
                fresh_shells,
                restored_previews
            );
            app.restore_report = Some(report.clone());
            persist_diag(&report);
            // 復元時のプレビューも background でハイライト / 読み込みする
            for (pane, path, text) in std::mem::take(&mut app.pending_highlights) {
                app.spawn_highlight(pane, path, text, cx);
            }
            app.drain_pending_preview_loads(cx);
        }
        // Web ビュー dock の退避分（ペイン無し）も初回 render で開き直す（#155）
        for url in webview_dock_restore {
            app.pending_webview_restore.push((None, url));
        }

        // Issue #191: layout.json に載っていない生存中の tmux セッション（orphan）を
        // 自動復帰する。kill -9 直前の spawn や crash で layout 保存が間に合わなかった
        // セッションを拾い、「復帰」タブにまとめて配置する。
        // セカンダリモード・persist OFF・tmux 不在では何もしない（= クリーン起動に影響なし）
        if tmux_persist && tmux_available && !secondary {
            let recovered = app.recover_orphan_sessions(cx);
            if !recovered.is_empty() {
                app.recovered_count = recovered.len();
                let report = format!(
                    "orphan 自動復帰: {} セッションを「復帰」タブへ追加",
                    recovered.len()
                );
                persist_diag(&report);
                eprintln!("info: {report}");
            }
        }

        // 起動時の orphan 一括クリーンアップ（FR-2.16.11）。復元で backend_sessions が
        // 出揃った後に実行し、前回クラッシュ等で取り残された detached・非 grouped の
        // backend セッションだけを掃除する（現存・バックグラウンド・表示中ビューは protected で除外）。
        // 直近 1 時間にアクティビティのあるセッションは対象外（Issue #113: layout.json が
        // 多重起動で巻き戻った場合に protected から漏れる実行中 worker を巻き込まない猶予。
        // 真の残骸はクラッシュから時間が経っており、次回以降の起動で掃除される）
        let cleaned = app.cleanup_orphan_tmux_with(Some(CLEANUP_STARTUP_GRACE_SECS));
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
                // Issue #168 / #115 / #181: サブプロセス実行（claude CLI / git）を伴う
                // read-only リクエスト（OrchestratorWorkerStatus / GitLog / GitDiff）は
                // UI スレッドで文脈収集だけ行い、実行と応答を background へ逃がす
                // （UI 非ブロック化 + 直列詰まりの解消。perf.log 実測の上位 2 種:
                // OrchestratorWorkerStatus avg 687ms×4124 回 / GitLog 2431ms。
                // #181 の worker_status_snapshot/compute 分離と同じ構造をここへ一本化）
                // TAKO_OFFLOAD=0 で従来の同期実行に戻せる（A/B 計測・問題切り分け用）
                let offload_enabled = !matches!(
                    std::env::var("TAKO_OFFLOAD").ok().as_deref(),
                    Some("0" | "false" | "off")
                );
                let offload = this.update(cx, |app: &mut TakoApp, _| {
                    if offload_enabled {
                        tako_control::prepare_offload(app, &incoming.request)
                    } else {
                        None
                    }
                });
                let Ok(offload) = offload else {
                    break; // View が破棄された
                };
                if let Some(prepared) = offload {
                    let reply = incoming.reply;
                    match prepared {
                        Ok(job) => {
                            cx.background_executor()
                                .spawn(async move {
                                    let _ = reply.send(job.run());
                                })
                                .detach();
                        }
                        Err(e) => {
                            let _ = reply.send(Err(e));
                        }
                    }
                    continue;
                }
                let result = this.update(cx, |app: &mut TakoApp, cx| {
                    // Issue #168: dispatch + 後処理（pending 消化 + save_layout）込みの
                    // IPC 1 件あたりのメインスレッド専有を計測（dispatch 単体とネスト計測）
                    let _span = tako_control::diag::perf_span("ipc_turn");
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
                    // 重量プレビュー（PDF / 動画）の background 読み込み（Issue #168）
                    app.drain_pending_preview_loads(cx);
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

        // PC 再起動では tmux プロセスも消えるため、実行中 Claude Code の session ID を
        // ペインごとに保存して `claude --resume` へ使う。外部コマンドは background で実行し、
        // 検出失敗時は直前の成功値を壊さない。既存 persist トグルが CLI / MCP 共通の制御点。
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(CLAUDE_SESSION_SCAN_INTERVAL)
                .await;
            let should_scan = this
                .update(cx, |app: &mut TakoApp, _| {
                    app.tmux_persist
                        && !app.secondary
                        && !app.backend_sessions.is_empty()
                        // self-test は外部 Claude CLI を呼ばず決定的に完走させる
                        && std::env::var_os("TAKO_SELF_TEST").is_none()
                })
                .unwrap_or(false);
            if !should_scan {
                continue;
            }
            // 1 回の `claude agents --json` 取得から resume マップ（従来）と
            // セッションカタログの検出（Issue #112 A）の両方を導出する
            let agents_value = cx
                .background_executor()
                .spawn(async { tako_control::agents::list_agents_with_panes(None) })
                .await;
            let Ok(agents_value) = agents_value else {
                continue;
            };
            let detected = tako_control::sessions::detect_from_agents_value(&agents_value);
            let resume_map: HashMap<String, String> = detected
                .iter()
                .map(|d| (d.tmux_session.clone(), d.session_id.clone()))
                .collect();
            let pane_meta = this.update(cx, |app: &mut TakoApp, _| {
                app.apply_claude_resume_sessions(&resume_map);
                app.save_layout();
                app.collect_pane_meta_snapshots()
            });
            let Ok(pane_meta) = pane_meta else {
                break;
            };
            // カタログの書き込み（ファイルロック + アトミック書き込み）は background で
            if !detected.is_empty() {
                cx.background_executor()
                    .spawn(async move {
                        if let Err(e) = tako_control::sessions::sync_detected(&detected, &pane_meta)
                        {
                            eprintln!("warning: セッションカタログの同期に失敗: {e}");
                        }
                    })
                    .await;
            }
        })
        .detach();

        // UI ストールウォッチドッグ（Issue #113 診断）: この async タスクは UI スレッド
        // （foreground executor）上で走るため、1 秒 timer からの再開遅延 = 「UI スレッドが
        // 他の処理で塞がっていた時間」になる。しきい値超えを perf.log に記録し、
        // 次に無応答が起きたとき時刻と長さがファイルに残るようにする（正常時は何も書かない）
        cx.spawn(async move |this, cx| loop {
            let t0 = std::time::Instant::now();
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let lag = t0.elapsed().saturating_sub(Duration::from_secs(1));
            if lag >= Duration::from_millis(500) {
                tako_control::diag::perf_log(&format!(
                    "UI ストール: イベントループ再開が {:.2}s 遅延",
                    lag.as_secs_f64()
                ));
            }
            // View 破棄でループ終了（他の定期ループと同じ生存判定）
            if this.update(cx, |_, _| {}).is_err() {
                break;
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

        // 2 秒毎の定期更新: tmux 一覧（FR-2.13）+ ファイルツリー（FR-3.1）+ git（FR-3.6）+
        // ペインログ（Issue #112 B）。外部コマンド実行は background で行い、
        // UI スレッドではコンテキスト収集と結果適用のみ
        cx.spawn(async move |this, cx| {
            let mut pane_log_tick: u32 = 0;
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                // ① main thread: tmux コンテキスト + filetree 対象 + view 監視対象 + git を収集（高速）
                let t0 = std::time::Instant::now();
                let prep = this.update(cx, |app: &mut TakoApp, _| {
                    // Issue #168: 定期更新の UI スレッド部（収集 + save_layout）を計測
                    let _span = tako_control::diag::perf_span("periodic_prep");
                    // ステップ別サブスパン（#212: periodic_prep の秒級スパイクをステップ単位で
                    // 攻撃者特定できるようにする。しきい値超えのみ記録 = 正常時コストほぼゼロ）
                    let tmux_ctx = {
                        let _s = tako_control::diag::perf_span("periodic_prep:tmux_ctx");
                        if app.panel_visible && app.panel_view == PanelView::Tmux {
                            Some(app.collect_tmux_context())
                        } else {
                            None
                        }
                    };
                    {
                        let _s = tako_control::diag::perf_span("periodic_prep:filetree_roots");
                        app.sync_filetree_roots();
                    }
                    {
                        let _s = tako_control::diag::perf_span("periodic_prep:agent_metrics");
                        app.refresh_agent_metrics();
                    }
                    {
                        let _s = tako_control::diag::perf_span("periodic_prep:webview");
                        app.poll_webview_state();
                    }
                    {
                        let _s = tako_control::diag::perf_span("periodic_prep:sleep_guard");
                        app.update_sleep_guard();
                    }
                    // 失敗遷移の検知 → Attention トースト（#217）
                    app.update_attention_toasts();
                    // ペインログ: 直接ペインはここで取り込み、バックエンドはジョブ化（Issue #112 B）
                    let log_jobs = {
                        let _s = tako_control::diag::perf_span("periodic_prep:pane_log");
                        app.collect_pane_log_work()
                    };
                    app.save_layout();
                    let filetree_targets = if app.filetree.visible {
                        let _s = tako_control::diag::perf_span("periodic_prep:filetree_targets");
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
                        app.git_cwd_for_tab()
                    } else {
                        None
                    };
                    let git_selected = app.git_selected_commit.clone();
                    // サイドバー表示中はブランチ + 変更サマリの軽量 git 取得（#217）
                    let sidebar_git_cwd = if app.filetree.visible {
                        app.git_cwd_for_tab()
                    } else {
                        None
                    };
                    (
                        tmux_ctx,
                        filetree_targets,
                        view_targets,
                        git_cwd,
                        git_selected,
                        sidebar_git_cwd,
                        log_jobs,
                        app.pane_logs.clone(),
                    )
                });
                // UI スレッド専有時間の計測（Issue #113 診断。しきい値超えのみ記録）
                let prep_ms = t0.elapsed().as_millis();
                if prep_ms >= 100 {
                    tako_control::diag::perf_log(&format!(
                        "定期更新（UI 部）遅延: 収集 + save_layout が {prep_ms}ms"
                    ));
                }
                let Ok((
                    tmux_ctx,
                    filetree_targets,
                    view_targets,
                    git_cwd,
                    git_selected,
                    sidebar_git_cwd,
                    log_jobs,
                    pane_logs,
                )) = prep
                else {
                    break;
                };
                // ② background: バックエンドペインのペインログ取り込み（probe + capture。
                // await して tick 内の順序を保つ = 同一ペインの増分が並行取り込みで重複しない）
                if !log_jobs.is_empty() {
                    let socket = tako_core::tmux_backend::socket_name();
                    let mgr = pane_logs.clone();
                    cx.background_executor()
                        .spawn(async move {
                            process_pane_log_jobs(&mgr, &socket, log_jobs);
                        })
                        .await;
                }
                // 全体上限の強制は低頻度（約 60 秒ごと）で十分
                pane_log_tick = pane_log_tick.wrapping_add(1);
                if pane_log_tick % 30 == 1 {
                    let mgr = pane_logs.clone();
                    cx.background_executor()
                        .spawn(async move {
                            let removed = {
                                let guard = mgr.lock().unwrap_or_else(|p| p.into_inner());
                                guard.enforce_total_cap()
                            };
                            if removed > 0 {
                                eprintln!(
                                    "info: ペインログの全体上限で {removed} ファイルを削除した"
                                );
                            }
                        })
                        .detach();
                }
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
                    // 構造の更新（メモリ操作）だけ UI スレッドで行い、window キャプチャ
                    // （tmux サブプロセス実行）は background へ逃がす（Issue #113: 旧実装は
                    // ここで window 数ぶん capture-pane を同期実行し、多 worker 時の定常的な
                    // UI ブロック源だった）
                    let targets = this.update(cx, |app: &mut TakoApp, cx| {
                        app.tmux_sessions = sessions;
                        let targets = app.sync_backend_windows();
                        cx.notify();
                        targets
                    });
                    let Ok(targets) = targets else {
                        break;
                    };
                    if !targets.is_empty() {
                        let socket = tako_core::tmux_backend::socket_name();
                        let captures = cx
                            .background_executor()
                            .spawn(async move {
                                targets
                                    .into_iter()
                                    .map(|(pane, session, win)| {
                                        let lines = tako_core::tmux::capture_pane_text(
                                            Some(&socket),
                                            &session,
                                            win,
                                        );
                                        ((pane, win), lines)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .await;
                        let ok = this.update(cx, |app: &mut TakoApp, cx| {
                            app.apply_window_captures(captures);
                            cx.notify();
                        });
                        if ok.is_err() {
                            break;
                        }
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
                // ④ background: サイドバー用の軽量 git サマリ（#217）
                if let Some(cwd) = sidebar_git_cwd {
                    let task = cx
                        .background_executor()
                        .spawn(async move { fetch_sidebar_git(&cwd) });
                    let data = task.await;
                    let ok = this.update(cx, |app: &mut TakoApp, cx| {
                        if app.sidebar_git != data {
                            app.sidebar_git = data;
                            cx.notify();
                        }
                    });
                    if ok.is_err() {
                        break;
                    }
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

        // アプリ内自動更新チェック（起動時 + 24 時間ごと。失敗時は静かにリトライ）
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            cx.spawn(async move |this, cx| loop {
                let task = cx
                    .background_executor()
                    .spawn(async { update_checker::check_latest() });
                let result = task.await;
                let wait = match result {
                    Ok(Some(info)) => {
                        let ok = this.update(cx, |app: &mut TakoApp, cx| {
                            if !matches!(app.update_state, update_checker::UpdateState::Dismissed) {
                                app.update_state = update_checker::UpdateState::Available(info);
                                cx.notify();
                            }
                        });
                        if ok.is_err() {
                            break;
                        }
                        update_checker::CHECK_INTERVAL
                    }
                    Ok(None) => {
                        // 既に最新 — CheckFailed 状態だったらクリアする
                        let _ = this.update(cx, |app: &mut TakoApp, cx| {
                            if matches!(
                                app.update_state,
                                update_checker::UpdateState::CheckFailed(_)
                            ) {
                                app.update_state = update_checker::UpdateState::Idle;
                                cx.notify();
                            }
                        });
                        update_checker::CHECK_INTERVAL
                    }
                    Err(e) => {
                        let retry = e.retry_duration();
                        let msg = e.to_string();
                        let _ = this.update(cx, |app: &mut TakoApp, cx| {
                            if !matches!(
                                app.update_state,
                                update_checker::UpdateState::Dismissed
                                    | update_checker::UpdateState::Available(_)
                            ) {
                                app.update_state = update_checker::UpdateState::CheckFailed(msg);
                                cx.notify();
                            }
                        });
                        retry
                    }
                };
                cx.background_executor().timer(wait).await;
            })
            .detach();
        }

        // ファイル監視の callback は OS バックエンドのスレッドから channel へ送るだけ。
        // イベントが無いアイドル時は UI スレッドへ一切起床・ポーリングを追加しない。
        app.sync_preview_watches();
        if let Some(mut rx) = preview_watch_rx {
            cx.spawn(async move |this, cx| {
                while let Some(signal) = rx.next().await {
                    if this
                        .update(cx, |app: &mut TakoApp, cx| {
                            app.handle_preview_watch_signal(signal, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }

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

    /// 提案チップの承諾（FR-2.4.3）。Web ビューペイン（FR-3.8 / #155）で検知元ペインの
    /// 隣にプレビューを開いてチップを畳む。webview を作れない場合は外部ブラウザへ
    /// フォールバックする
    fn accept_port_suggestion(&mut self, pane: PaneId, port: u16, cx: &mut Context<Self>) {
        self.port_suggestions
            .retain(|s| !(s.pane == pane && s.port == port));
        let url = format!("http://localhost:{port}");
        // セルフテスト中は実ページを開かずチップの状態遷移だけ検証する（既存方針）
        if std::env::var_os("TAKO_SELF_TEST").is_some() {
            cx.notify();
            return;
        }
        let opened = tako_control::dispatch(
            self,
            tako_control::protocol::Request::Web {
                action: "open".into(),
                url: Some(url.clone()),
                id: None,
                pane: Some(pane.as_u64()),
                direction: None,
                to: None,
                js: None,
                token: None,
                focus: Some(true),
            },
            PaneOrigin::User,
        );
        if let Err(e) = opened {
            eprintln!("warning: Web ビューを開けないため外部ブラウザへ委譲: {e}");
            open_preview(&url);
        }
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
                                Some(preview) => preview.file_name().to_string(),
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
                // プレビューペインならファイル名をラベルにする（#230）
                self.previews
                    .get(&p.id())
                    .and_then(|s| s.path.file_name())
                    .map(|n| n.to_string_lossy().to_string())
            })
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
        if self.previews.contains_key(&pane_id) {
            return CommandState::Idle;
        }
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

    /// どのタブにも表示されていない tmux セッションの抽出（FR-2.16.8 / #183）。
    /// 対応ペインを持つバックエンドセッションはタブ枠内のペイン行が、tako ペイン内で
    /// attach 中のセッションはタブ枠内の紐付け表示（FR-2.16.9）が代表するため除外し、
    /// 残りを「kill 漏れ?（orphan バックエンド）」と「管理外（ユーザー直起動等）」に分類する。
    /// #183: ロール/cwd/プロセス/最終アクティビティを表示、orphan 判定を改良
    fn tmux_unlisted_sessions(&self) -> Vec<UnlistedTmuxSession> {
        let bg_sessions: std::collections::HashSet<String> = self
            .workspace
            .shelved_panes()
            .iter()
            .filter_map(|p| self.backend_sessions.get(&p.id()).cloned())
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.tmux_sessions
            .iter()
            .filter_map(|session| {
                let backend = session["backend"].as_bool().unwrap_or(false);
                if backend && session["backend_pane"].as_u64().is_some() {
                    return None;
                }
                if let Some(name) = session["name"].as_str() {
                    if name.starts_with("tako-view-") {
                        return None;
                    }
                    if bg_sessions.contains(name) {
                        return None;
                    }
                }
                if Self::tmux_session_attached_at(session).is_some() {
                    return None;
                }
                let windows = Self::tmux_session_windows(session);
                let name_str = session["name"].as_str().unwrap_or("?").to_string();
                let role = Self::infer_role_from_session_name(&name_str);
                let process = session["pane_command"].as_str().unwrap_or("").to_string();
                let cwd = session["pane_current_path"]
                    .as_str()
                    .map(|p| {
                        if let Ok(home) = std::env::var("HOME") {
                            if let Some(rest) = p.strip_prefix(&home) {
                                return format!("~{rest}");
                            }
                        }
                        p.to_string()
                    })
                    .unwrap_or_default();
                let last_activity = session["last_activity"].as_i64().unwrap_or(0);
                let last_activity_age = if last_activity > 0 {
                    format_age(now - last_activity)
                } else {
                    String::new()
                };
                Some(UnlistedTmuxSession {
                    name: name_str,
                    socket: session["socket"].as_str().map(str::to_string),
                    orphan_backend: backend,
                    attached: session["attached"].as_bool().unwrap_or(false),
                    windows,
                    role,
                    process,
                    cwd,
                    last_activity_age,
                })
            })
            .collect()
    }

    /// セッション名から TAKO_ORCHESTRATOR_ROLE を推定する（#183）。
    /// バックエンドセッション名は「orchestrator-worker:tako:167-mouse-leak」等の形式
    fn infer_role_from_session_name(name: &str) -> String {
        if let Some(rest) = name.strip_prefix("orchestrator-") {
            return rest.to_string();
        }
        if name.starts_with("master") || name.starts_with("worker") || name.starts_with("solo") {
            return name.to_string();
        }
        String::new()
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
    /// UI のピンボタンと dispatch（CLI / MCP）の両方から呼ぶ（操作経路を一本化）。
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
        // 構造のみ即時更新。window キャプチャ（サブプロセス実行）は次の 2 秒 tick が
        // background で埋める（UI スレッドで tmux を同期実行しない。Issue #113）
        let _ = self.sync_backend_windows();
    }

    /// tmux_sessions JSON からバックエンドペインの window 一覧を抽出する（メモリ操作のみ）。
    /// 2+ window のセッションのみ backend_windows に保持し、ホバープレビュー用に
    /// キャプチャすべき非アクティブ window（pane, session, window index）を返す。
    /// capture-pane はサブプロセス実行のため呼び出し側が background で行い、結果を
    /// `apply_window_captures` で適用する（Issue #113: 旧実装はここで UI スレッドの
    /// 同期実行をしており、多 worker = 多 window 時の定常的な UI ブロック源だった）
    fn sync_backend_windows(&mut self) -> Vec<(PaneId, String, u32)> {
        self.backend_windows.clear();
        let mut capture_targets = Vec::new();
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
                        capture_targets.push((*pane_id, session_name.clone(), w.index));
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
        capture_targets
    }

    /// background で採取した window キャプチャを適用する（`sync_backend_windows` と対）。
    /// 採取中に対象が消えていたら捨てる（retain と同じ整合条件）
    fn apply_window_captures(&mut self, captures: Vec<((PaneId, u32), Vec<String>)>) {
        for ((pane, win), lines) in captures {
            if self
                .backend_windows
                .get(&pane)
                .is_some_and(|ws| ws.iter().any(|w| w.index == win))
            {
                self.window_captures.insert((pane, win), lines);
            }
        }
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

    /// git タブ用の cwd を決定する。フォーカスペインの cwd を優先し、
    /// git リポジトリが見つからなければファイルツリーの全ソース（他ペインの cwd +
    /// pinned フォルダ）からフォールバック検索する（#313）
    fn git_cwd_for_tab(&self) -> Option<std::path::PathBuf> {
        let tab = self.workspace.active_tab();
        let active_pane = tab.tree().focused();

        // フォーカスペインの cwd が git リポジトリ内ならそれを使う
        if let Some(cwd) = self.terminals.get(&active_pane).and_then(|s| s.cwd()) {
            if has_git_ancestor(cwd) {
                return Some(cwd.to_path_buf());
            }
        }

        // 他のフォアグラウンドペインの cwd を走査
        for pane in tab.tree().panes() {
            if pane.id() == active_pane {
                continue;
            }
            if let Some(cwd) = self.terminals.get(&pane.id()).and_then(|s| s.cwd()) {
                if has_git_ancestor(cwd) {
                    return Some(cwd.to_path_buf());
                }
            }
        }

        // pinned フォルダを走査
        for folder in tab.pinned_folders() {
            if has_git_ancestor(folder) {
                return Some(folder.clone());
            }
        }

        // バックグラウンドペイン（同タブ由来）を走査
        let tab_id = tab.id();
        for bp in self.workspace.shelved_panes() {
            if bp.origin_tab() != tab_id {
                continue;
            }
            if let Some(cwd) = self.terminals.get(&bp.id()).and_then(|s| s.cwd()) {
                if has_git_ancestor(cwd) {
                    return Some(cwd.to_path_buf());
                }
            }
        }

        None
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

    /// claude TUI へのプロンプト送達フローを駆動する（Issue #32 送達確認ループ）。
    /// 画面内容を確認しながら各ステップを進める（sleep ベースではない）。
    /// 検出ロジックは実 TUI の採取画面に基づく `tako_control::claude_tui` を使う
    fn drive_prompt_flows(&mut self) {
        use tako_control::claude_tui;
        let mut remaining = Vec::new();
        // 同一ペインへ複数フローが重なると貼り付けと Enter が混線するため、
        // 先行フローが完了するまで後続は待たせる（Vec の順序 = 送信順）
        let mut active_panes: std::collections::HashSet<PaneId> = std::collections::HashSet::new();
        let now = std::time::Instant::now();
        for mut flow in std::mem::take(&mut self.prompt_flows) {
            if flow.created_at.elapsed() > std::time::Duration::from_secs(120) {
                eprintln!(
                    "warning: プロンプト送達フローがタイムアウト（pane={}）",
                    flow.pane.as_u64()
                );
                continue;
            }
            if active_panes.contains(&flow.pane) {
                remaining.push(flow);
                continue;
            }
            let session = match self.terminals.get(&flow.pane) {
                Some(s) => s,
                None => {
                    active_panes.insert(flow.pane);
                    remaining.push(flow);
                    continue;
                }
            };
            match flow.state {
                PromptFlowState::WaitAltScreen => {
                    // agy 1.1.0 は inline モード（非 alt_screen）で動くため、alt_screen 遷移
                    // だけを待つと永遠に進まない（#120）。入力欄 / 信頼ダイアログが画面に
                    // 見えたら即進み、どちらも来なければ 15 秒で先へ進む（未知 TUI 耐性）
                    let lines = session.visible_lines();
                    if session.is_alt_screen()
                        || claude_tui::is_trust_dialog(&lines)
                        || claude_tui::input_line(&lines).is_some()
                        || flow.state_entered_at.elapsed() > std::time::Duration::from_secs(15)
                    {
                        flow.state = PromptFlowState::WaitPromptReady;
                        flow.state_entered_at = now;
                    }
                }
                PromptFlowState::WaitPromptReady => {
                    let lines = session.visible_lines();
                    if claude_tui::is_trust_dialog(&lines) {
                        // 信頼ダイアログがプロンプトを消費するのを防ぐ: 先に Enter で承諾する
                        // （事前信頼が書けなかった場合のフォールバック）。tick 間隔 500ms が
                        // 承諾間の自然な遅延になる。3 回で打ち切り（総合タイムアウトに委ねる）
                        if flow.trust_accepts < 3 {
                            session.write(b"\r".to_vec());
                            flow.trust_accepts += 1;
                            flow.state_entered_at = now;
                        }
                    } else if flow.enter_only {
                        // Enter 単独送達（Issue #95）: 貼り付けせず、入力欄の現内容を
                        // 残留判定の基準に控えて Enter を送る。プロンプト記号が見えない画面
                        // （他 TUI 等）へも 2 秒待って Enter だけ送る（検証は不能 = 1 発）
                        if claude_tui::input_line(&lines).is_some()
                            || flow.state_entered_at.elapsed() > std::time::Duration::from_secs(2)
                        {
                            flow.baseline = claude_tui::input_line(&lines)
                                .filter(|s| !claude_tui::input_content_is_empty(s))
                                .map(str::to_string);
                            session.write(b"\r".to_vec());
                            flow.state = PromptFlowState::VerifySubmitted;
                            flow.state_entered_at = now;
                        }
                    } else if claude_tui::input_line(&lines).is_some()
                        || (!flow.wait_tui
                            && flow.state_entered_at.elapsed() > std::time::Duration::from_secs(2))
                    {
                        // 入力欄（プロンプト記号）を確認して貼り付け。汎用送信（wait_tui=false）は
                        // 対象が claude TUI でなくても 2 秒待って貼る（他 TUI への送信）。
                        // bracketed paste はアプリが要求していれば paste() が括りを付ける
                        session.paste(&flow.prompt);
                        flow.state = PromptFlowState::WaitTextInInput;
                        flow.state_entered_at = now;
                    }
                }
                PromptFlowState::WaitTextInInput => {
                    let lines = session.visible_lines();
                    let head = claude_tui::prompt_head(&flow.prompt);
                    // 入力欄への反映（マルチラインは [Pasted text #N] 表示）を確認。
                    // 折り返し・全角幅で入力欄行にマッチしないケースは画面全体 or
                    // 10 秒タイムアウトで救済（従来挙動の維持）
                    let reflected = claude_tui::text_in_input(&lines, &flow.prompt)
                        || (!head.is_empty() && lines.iter().any(|l| l.contains(head.as_str())));
                    let timed_out =
                        flow.state_entered_at.elapsed() > std::time::Duration::from_secs(10);
                    if reflected || timed_out {
                        // 送信の Enter は貼り付けと分離した単独キーとして送る
                        // （貼り付けバーストに混ざると「次の行」と解釈される）
                        session.write(b"\r".to_vec());
                        flow.state = PromptFlowState::VerifySubmitted;
                        flow.state_entered_at = now;
                    }
                }
                PromptFlowState::VerifySubmitted => {
                    // Enter の画面反映を 1 tick 待ってから検証する
                    if flow.state_entered_at.elapsed() >= std::time::Duration::from_millis(400) {
                        let lines = session.visible_lines();
                        let residual = if flow.enter_only {
                            // Enter 単独送達（Issue #95）: 基準と同じ非空テキストが
                            // 入力欄に残っている = Enter 未送達。空 / プレースホルダ /
                            // 別内容（ユーザーが打ち直した等）なら完了とみなし干渉しない
                            match (claude_tui::input_line(&lines), flow.baseline.as_deref()) {
                                (Some(content), Some(base)) => content == base,
                                _ => false,
                            }
                        } else {
                            claude_tui::input_residual(&lines, &flow.prompt)
                        };
                        if !residual {
                            // 入力欄が空へ戻った = 送信された
                            flow.state = PromptFlowState::Done;
                        } else if flow.enter_retries_left > 0 {
                            // Enter が「送信」と解釈されず残留 → Enter を単独再送
                            session.write(b"\r".to_vec());
                            flow.enter_retries_left -= 1;
                            flow.state_entered_at = now;
                        } else {
                            eprintln!(
                                "warning: プロンプト送達を検証できない（pane={} 入力欄に残留）",
                                flow.pane.as_u64()
                            );
                            flow.state = PromptFlowState::Done;
                        }
                    }
                }
                PromptFlowState::Done => {}
            }
            if !matches!(flow.state, PromptFlowState::Done) {
                active_panes.insert(flow.pane);
                remaining.push(flow);
            }
        }
        self.prompt_flows = remaining;
    }

    /// 人間の Enter の送達検証フロー（Issue #95）を登録する。同一ペインに
    /// enter_only フローが既にあれば積まない（連打対策。先行フローの再送が代表する）
    fn queue_enter_verify(&mut self, pane: PaneId, baseline: String) {
        if self
            .prompt_flows
            .iter()
            .any(|f| f.pane == pane && f.enter_only)
        {
            return;
        }
        self.prompt_flows
            .push(PromptFlow::new_enter_verify(pane, baseline));
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
                force: true,
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
        let mut need_immediate = false;
        match session.process_event(event) {
            Some(SessionNotice::Exited) => {
                // PTY 死亡由来の close: セッション kill・layout 削除はしない（Issue #30）
                self.remove_pane_with(pane_id, CloseReason::Exited, cx);
                need_immediate = true;
            }
            Some(SessionNotice::ClipboardStore(text)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                need_immediate = true;
            }
            Some(SessionNotice::TitleChanged) | None => {}
        }
        // 16ms（~60fps）のフレームレート制限: 重要イベントは即座に再描画し、
        // 通常の出力データは最大 16ms 遅延させてバッチ化する
        if need_immediate {
            self.last_term_notify = std::time::Instant::now();
            cx.notify();
            return;
        }
        let elapsed = self.last_term_notify.elapsed();
        if elapsed >= Duration::from_millis(16) {
            self.last_term_notify = std::time::Instant::now();
            cx.notify();
        } else if !self.term_notify_pending {
            self.term_notify_pending = true;
            let remaining = Duration::from_millis(16) - elapsed;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(remaining).await;
                let _ = this.update(cx, |app, cx| {
                    app.term_notify_pending = false;
                    app.last_term_notify = std::time::Instant::now();
                    cx.notify();
                });
            })
            .detach();
        }
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
    fn set_preview_zoom_about(
        &mut self,
        pane_id: PaneId,
        zoom: f32,
        center: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = <Self as PreviewHost>::preview_view_state(self, pane_id) else {
            return;
        };
        let zoom = zoom.clamp(tako_core::PREVIEW_ZOOM_MIN, tako_core::PREVIEW_ZOOM_MAX);
        let pan_delta = self
            .preview_scroll_handles
            .get(&pane_id)
            .and_then(|handle| {
                let bounds = handle.bounds();
                if f32::from(bounds.size.width) <= 0.0 || f32::from(bounds.size.height) <= 0.0 {
                    return None;
                }
                let center = center.unwrap_or_else(|| {
                    point(
                        bounds.origin.x + bounds.size.width / 2.0,
                        bounds.origin.y + bounds.size.height / 2.0,
                    )
                });
                let relative_x = f32::from(center.x - bounds.origin.x);
                let relative_y = f32::from(center.y - bounds.origin.y);
                let ratio = zoom / current.zoom.max(f32::EPSILON);
                // update_preview_view が現在パンを倍率比で拡大するため、ここでは
                // ズーム中心を画面上の同じ位置へ保つ追加差分だけを渡す。
                Some((relative_x * (ratio - 1.0), relative_y * (ratio - 1.0)))
            });
        if <Self as PreviewHost>::update_preview_view(
            self,
            pane_id,
            tako_core::PreviewViewUpdate {
                zoom: Some(tako_core::PreviewZoomCommand::Set(zoom)),
                pan_delta,
                ..tako_core::PreviewViewUpdate::default()
            },
        )
        .is_ok()
        {
            cx.notify();
        }
    }

    /// フォーカス中ペインのフォントサイズを delta 分だけ変更する。
    /// PDF・画像プレビューでは同じキーをコンテンツズームへ割り当てる。
    fn zoom_focused_pane(&mut self, delta: f32, cx: &mut Context<Self>) {
        let pane_id = self.focused_pane();
        if let Some(view) = <Self as PreviewHost>::preview_view_state(self, pane_id) {
            let factor = if delta >= 0.0 {
                tako_core::PREVIEW_ZOOM_STEP
            } else {
                1.0 / tako_core::PREVIEW_ZOOM_STEP
            };
            self.set_preview_zoom_about(pane_id, view.zoom * factor, None, cx);
            return;
        }
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
        if <Self as PreviewHost>::preview_view_state(self, pane_id).is_some() {
            if <Self as PreviewHost>::update_preview_view(
                self,
                pane_id,
                tako_core::PreviewViewUpdate {
                    zoom: Some(tako_core::PreviewZoomCommand::Reset),
                    ..tako_core::PreviewViewUpdate::default()
                },
            )
            .is_ok()
            {
                cx.notify();
            }
            return;
        }
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

    /// × ボタン close の確認ダイアログ付きハンドラ（Issue #172）。
    /// `cmd_held` = true なら確認スキップ（パワーユーザー動線）
    fn close_pane_with_confirm(&mut self, pane_id: PaneId, cmd_held: bool, cx: &mut Context<Self>) {
        if cmd_held || !self.confirm_close {
            self.close_pane_button(pane_id, cx);
        } else {
            self.pending_close_confirm = Some(CloseConfirmTarget::Pane(pane_id));
            cx.notify();
        }
    }

    /// タブの × ボタン close の確認ダイアログ付きハンドラ（Issue #172）
    fn close_tab_with_confirm(&mut self, tab_id: TabId, cmd_held: bool, cx: &mut Context<Self>) {
        if cmd_held || !self.confirm_close {
            self.remove_tab(tab_id, cx);
        } else {
            self.pending_close_confirm = Some(CloseConfirmTarget::Tab(tab_id));
            cx.notify();
        }
    }

    /// 確認ダイアログで「閉じる」が選ばれたとき
    fn close_confirm_accepted(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.pending_close_confirm.take() else {
            return;
        };
        match target {
            CloseConfirmTarget::Pane(id) => self.close_pane_button(id, cx),
            CloseConfirmTarget::Tab(id) => self.remove_tab(id, cx),
        }
        cx.notify();
    }

    /// 確認ダイアログでキャンセルされたとき
    fn close_confirm_cancelled(&mut self, cx: &mut Context<Self>) {
        self.pending_close_confirm = None;
        cx.notify();
    }

    /// タブ/ペインの close で「失われるもの」の要約を生成する（Issue #172）
    fn close_summary(&self, target: CloseConfirmTarget) -> String {
        match target {
            CloseConfirmTarget::Pane(pane_id) => {
                let mut parts = Vec::new();
                if let Some(session) = self.terminals.get(&pane_id) {
                    if session.command_state() == CommandState::Running {
                        parts.push("実行中のプロセス".to_string());
                    }
                }
                if let Some(pane) = self
                    .workspace
                    .tabs()
                    .iter()
                    .flat_map(|t| t.tree().panes())
                    .find(|p| p.id() == pane_id)
                {
                    if let Some(role) = pane.role() {
                        if role.starts_with("orchestrator-worker") {
                            let busy = self
                                .terminals
                                .get(&pane_id)
                                .is_some_and(|s| s.command_state() == CommandState::Running);
                            if busy {
                                parts.push("稼働中の worker".to_string());
                            }
                        }
                    }
                }
                if self.backend_sessions.contains_key(&pane_id) {
                    parts.push("tmux セッション".to_string());
                }
                if parts.is_empty() {
                    "このペインを閉じますか？".to_string()
                } else {
                    format!("閉じると失われるもの: {}", parts.join("、"))
                }
            }
            CloseConfirmTarget::Tab(tab_id) => {
                let Some(tab) = self.workspace.get_tab(tab_id) else {
                    return "このタブを閉じますか？".to_string();
                };
                let pane_count = tab.tree().panes().len();
                let running = tab
                    .tree()
                    .panes()
                    .iter()
                    .filter(|p| {
                        self.terminals
                            .get(&p.id())
                            .is_some_and(|s| s.command_state() == CommandState::Running)
                    })
                    .count();
                let workers = tab
                    .tree()
                    .panes()
                    .iter()
                    .filter(|p| {
                        p.role()
                            .is_some_and(|r| r.starts_with("orchestrator-worker"))
                            && self
                                .terminals
                                .get(&p.id())
                                .is_some_and(|s| s.command_state() == CommandState::Running)
                    })
                    .count();
                let tmux = tab
                    .tree()
                    .panes()
                    .iter()
                    .filter(|p| self.backend_sessions.contains_key(&p.id()))
                    .count();

                let mut parts = Vec::new();
                parts.push(format!("{pane_count} ペイン"));
                if running > 0 {
                    parts.push(format!("{running} 個の実行中プロセス"));
                }
                if workers > 0 {
                    parts.push(format!("{workers} 個の稼働中 worker"));
                }
                if tmux > 0 {
                    parts.push(format!("{tmux} 個の tmux セッション"));
                }
                format!("閉じると失われるもの: {}", parts.join("、"))
            }
        }
    }

    /// ペインタイトルバーの ー ボタン = ペインをバックグラウンドへバックグラウンド（FR-2.15.1）。
    /// プロセス・tmux セッションは生かしたまま、ツリーから外してバックグラウンドに移す。
    /// 最後のタブの最後のペインのときは代替ペインを生やしてからバックグラウンドする
    fn background_pane_button(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        // Web ビューペインはターミナル用のたまり場ではなく Web ビュー dock へ退避する
        // （たまり場はスクリーンサムネイル前提で、webview には端末画面が無い）
        if self.webviews.iter().any(|e| e.pane == Some(pane_id)) {
            self.webview_hide_button(pane_id, cx);
            return;
        }
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
        self.sync_preview_watches();
        cx.notify();
    }

    /// タブ内の全ペインをバックグラウンドへバックグラウンドする（FR-2.15 タブ単位バックグラウンド）
    fn background_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        match self.workspace.shelve_tab(tab_id) {
            Ok(_shelved_ids) => {}
            Err(WorkspaceError::LastTab) => {
                let new_pane = Pane::new(PaneOrigin::User);
                let new_id = new_pane.id();
                let title = (self.workspace.tabs().len() + 1).to_string();
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
        self.sync_preview_watches();
        cx.notify();
    }

    /// BG 復帰時のプレビュー監視再開 + リロードトリガー（#230）。
    /// dispatch 経由（Foreground）は ControlHost::reattach_backgrounded で、
    /// ドロワー / D&D はこちらを直接呼ぶ
    pub(crate) fn reattach_backgrounded_preview(&mut self, pane: PaneId) {
        if let Some(state) = self.previews.get(&pane) {
            let path = state.path.clone();
            let mode = state.mode;
            self.sync_preview_watches();
            if preview::live_reload_supported(mode) {
                self.pending_preview_loads.push((pane, path, mode));
            }
        }
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
        self.remove_pane_with(pane_id, CloseReason::Explicit, cx);
    }

    /// ペインを閉じたときのバックエンドセッション後始末を理由で出し分ける（Issue #30）。
    /// 明示 close = kill（孤児を残さない）。PTY 死亡 = 登録だけ外して kill しない
    /// （シェル exit なら既にセッションは消えており、クライアント kick / サーバー異常なら
    /// セッションは生きているか他インスタンスが引き継いでいる。残骸は起動時の
    /// orphan クリーンアップ（FR-2.16.11）と tmux ビューの「kill漏れ?」表示が拾う）
    fn drop_backend_session_with(&mut self, pane_id: PaneId, reason: CloseReason) {
        self.claude_resume_sessions.remove(&pane_id);
        match reason {
            CloseReason::Explicit => self.drop_backend_session(pane_id),
            CloseReason::Exited => {
                self.backend_sessions.remove(&pane_id);
            }
        }
    }

    fn remove_pane_with(&mut self, pane_id: PaneId, reason: CloseReason, cx: &mut Context<Self>) {
        let Some(tab_id) = self
            .workspace
            .tabs()
            .iter()
            .find(|t| t.tree().contains(pane_id))
            .map(|t| t.id())
        else {
            return;
        };
        // ペインログの最終フラッシュ素材（Issue #112 B）。close 成功後に書き込む
        // （LastPane 分岐は remove_tab_with 側が全ペイン分をフラッシュする）
        let log_close = self.pane_log_close_data(pane_id);
        let tab = self
            .workspace
            .get_tab_mut(tab_id)
            .expect("直前に存在を確認したタブ");
        // Issue #165: worker close 後のリフロー用に spawn 元を close 前に記録する
        // （明示 close も worker プロセスの exit 由来も対象）
        let reflow_anchor = tab
            .tree()
            .get(pane_id)
            .filter(|p| {
                p.role()
                    .is_some_and(|r| r.starts_with("orchestrator-worker"))
            })
            .and_then(|p| p.spawned_by());
        match tab.tree_mut().close(pane_id) {
            Ok(_) => {
                // Issue #165: worker が抜けた領域を残りの worker で再配分する
                // （master・ユーザー由来ペインの矩形は変わらない）
                if let Some(anchor) = reflow_anchor {
                    let layout = tako_control::setup::spawn_layout_config();
                    if layout.policy != tako_core::SpawnLayoutPolicy::Legacy {
                        let _ = tab.tree_mut().reflow_workers(anchor, layout.algorithm);
                    }
                }
                // ペインログの最終フラッシュ（Issue #112 B。セッション破棄前に書き残す）
                if let Some(data) = log_close {
                    self.apply_pane_log_close(pane_id, data, reason);
                }
                self.terminals.remove(&pane_id);
                self.previews.remove(&pane_id);
                self.preview_edits.remove(&pane_id);
                self.video_players.remove(&pane_id);
                self.remove_video_frame_cache(pane_id);
                self.remove_preview_image_cache(pane_id);
                self.preview_changelogs.remove(&pane_id);
                self.preview_views.remove(&pane_id);
                self.preview_scroll_handles.remove(&pane_id);
                self.pending_pdf_rasters.remove(&pane_id);
                self.active_pdf_rasters.remove(&pane_id);
                self.video_seek_bar_bounds.remove(&pane_id);
                self.preview_selections.remove(&pane_id);
                self.preview_line_bounds.remove(&pane_id);
                self.preview_pdf_char_bounds.remove(&pane_id);
                self.preview_pdf_highlight_paint_count.remove(&pane_id);
                self.preview_pdf_page_image_bounds.remove(&pane_id);
                self.preview_text_layouts.remove(&pane_id);
                self.preview_line_texts.remove(&pane_id);
                self.pane_links.remove(&pane_id);
                self.known_failed.remove(&pane_id);
                self.scroll_accum.remove(&pane_id);
                self.scroll_ctls.remove(&pane_id);
                self.pane_font_sizes.remove(&pane_id);
                self.pane_cell_sizes.remove(&pane_id);
                self.dock_webview_of(pane_id);
                self.drop_tmux_view_session(pane_id);
                self.drop_backend_session_with(pane_id, reason);
            }
            Err(_) => {
                // LastPane: タブごと閉じる
                self.remove_tab_with(tab_id, reason, cx);
            }
        }
        self.sync_preview_watches();
        cx.notify();
    }

    fn remove_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        self.remove_tab_with(tab_id, CloseReason::Explicit, cx);
    }

    fn remove_tab_with(&mut self, tab_id: TabId, reason: CloseReason, cx: &mut Context<Self>) {
        let Some(tab) = self.workspace.get_tab(tab_id) else {
            return;
        };
        let pane_ids: Vec<PaneId> = tab.tree().panes().iter().map(|p| p.id()).collect();
        // ペインログの最終フラッシュ（Issue #112 B）。タブ close / 全ペイン終了の両分岐で
        // 素材を close 前に採取し、どちらの経路でも書き残す
        let log_closes: Vec<(PaneId, PaneLogCloseData)> = pane_ids
            .iter()
            .filter_map(|id| self.pane_log_close_data(*id).map(|d| (*id, d)))
            .collect();
        match self.workspace.close_tab(tab_id) {
            Ok(_) => {
                for (id, data) in log_closes {
                    self.apply_pane_log_close(id, data, reason);
                }
                for id in pane_ids {
                    self.terminals.remove(&id);
                    self.previews.remove(&id);
                    self.preview_edits.remove(&id);
                    self.video_players.remove(&id);
                    self.remove_video_frame_cache(id);
                    self.remove_preview_image_cache(id);
                    self.preview_views.remove(&id);
                    self.preview_scroll_handles.remove(&id);
                    self.pending_pdf_rasters.remove(&id);
                    self.active_pdf_rasters.remove(&id);
                    self.video_seek_bar_bounds.remove(&id);
                    self.preview_selections.remove(&id);
                    self.preview_line_bounds.remove(&id);
                    self.preview_pdf_char_bounds.remove(&id);
                    self.preview_pdf_highlight_paint_count.remove(&id);
                    self.preview_pdf_page_image_bounds.remove(&id);
                    self.preview_text_layouts.remove(&id);
                    self.preview_line_texts.remove(&id);
                    self.pane_links.remove(&id);
                    self.known_failed.remove(&id);
                    self.scroll_accum.remove(&id);
                    self.scroll_ctls.remove(&id);
                    self.dock_webview_of(id);
                    self.drop_tmux_view_session(id);
                    self.drop_backend_session_with(id, reason);
                }
            }
            Err(_) => {
                // LastTab: アプリ終了は UI 層の責務。
                // 明示 close = セッションも破棄し、次回起動で空レイアウトを復元しないよう
                // ファイルも消す。PTY 死亡由来（tmux サーバー死・クライアント kick・
                // シェル exit）= セッションは kill せず layout.json も保持し、
                // 次回起動でタブ構成を復元できるようにする（Issue #30。
                // 2026-07-03 実機: サーバー死で全タブが道連れ削除された）
                //
                // 冪等化ラッチ（Issue #113）: この分岐は最後のペインを workspace /
                // terminals から取り除かないため、同一 PTY の Exit と ChildExit
                // （terminal.rs で両方 SessionNotice::Exited になる）が二重に届くと
                // ここを 2 回通り、「全ペイン終了」ログと quit が重複発火していた
                // （実機 persist.log の同時刻二重行の正体）
                if self.quitting {
                    return;
                }
                self.quitting = true;
                for id in pane_ids {
                    self.drop_tmux_view_session(id);
                    self.drop_backend_session_with(id, reason);
                }
                // ペインログの最終フラッシュ（Issue #112 B。アプリ終了直前に書き残す）
                for (id, data) in log_closes {
                    self.apply_pane_log_close(id, data, reason);
                }
                // セカンダリモードは layout.json の所有者ではないため削除もログも行わない
                // （プライマリの復元情報を道連れにしない。Issue #113）
                if std::env::var_os("TAKO_SELF_TEST").is_none() && !self.secondary {
                    match reason {
                        CloseReason::Explicit => {
                            tako_control::layout::remove();
                            persist_diag(
                                "layout.json 削除: 最後のペインの明示クローズ（次回は空で起動）",
                            );
                        }
                        CloseReason::Exited => {
                            persist_diag(
                                "全ペイン終了（PTY 死亡）: layout.json は保持して終了（次回起動で復元）",
                            );
                        }
                    }
                }
                // 明示 close は空で再開するため接続情報も片付ける。PTY 死亡由来は
                // ⌘Q と同様、persist ON なら次回の同一ソケット再接続のため残す
                if reason == CloseReason::Explicit || !self.tmux_persist {
                    tako_control::discovery::cleanup(std::process::id());
                }
                cx.quit();
            }
        }
        self.sync_preview_watches();
        cx.notify();
    }

    /// orphan tmux セッションの一括クリーンアップ本体（FR-2.16.11）。
    /// `min_idle_secs` は起動時の自動実行だけが渡す猶予（Issue #113: 直前まで動いていた
    /// セッションを protected 漏れ時にも巻き込まない）。判定は `cleanup_orphans` を参照
    /// layout.json に載っていない生存中 tmux セッション（orphan）を発見し、
    /// 「復帰」タブにまとめて自動追加する（Issue #191）。
    /// spawn_session を通すため backend_sessions に登録され、
    /// 後続の cleanup_orphan_tmux からは protected として保護される
    fn recover_orphan_sessions(&mut self, cx: &mut Context<Self>) -> Vec<String> {
        let protected: std::collections::HashSet<String> =
            self.backend_sessions.values().cloned().collect();
        let socket = tako_core::tmux_backend::socket_name();
        let orphans = tako_core::tmux_backend::find_orphans(&socket, &protected);
        if orphans.is_empty() {
            return Vec::new();
        }
        // orphan ごとに cwd と role を取得
        let tab_title = "復帰".to_string();
        let first_pane = Pane::new(PaneOrigin::User);
        let first_id = first_pane.id();
        self.workspace.create_tab(tab_title, first_pane);
        let tab_id = self.workspace.find_tab_of_pane(first_id).unwrap();
        // 最初の orphan を最初のペインに割り当て、残りは分割で追加
        let mut recovered = Vec::new();
        for (i, name) in orphans.iter().enumerate() {
            let pane_id = if i == 0 {
                first_id
            } else {
                let new_pane = Pane::new(PaneOrigin::User);
                let new_id = new_pane.id();
                let _ = self.workspace.active_tab_mut().tree_mut().split(
                    first_id,
                    tako_core::SplitDirection::Down,
                    new_pane,
                );
                new_id
            };
            self.backend_sessions.insert(pane_id, name.clone());
            let cwd = tako_core::tmux_backend::session_cwd(&socket, name);
            let options = SpawnOptions {
                cwd: cwd.map(std::path::PathBuf::from).filter(|p| p.is_dir()),
                ..SpawnOptions::default()
            };
            if let Err(e) = self.spawn_session(pane_id, options, cx) {
                eprintln!("warning: orphan セッション {name} を復帰できない: {e}");
                self.backend_sessions.remove(&pane_id);
                continue;
            }
            // #210: orphan セッションの TAKO_ORCHESTRATOR_ROLE から role を引き継ぐ
            let role =
                tako_core::tmux_backend::session_env(&socket, name, "TAKO_ORCHESTRATOR_ROLE")
                    .and_then(|env_role| role_from_orchestrator_env(&env_role));
            if let Some(role) = &role {
                if let Some(pane_obj) = self
                    .workspace
                    .get_tab_mut(tab_id)
                    .and_then(|t| t.tree_mut().get_mut(pane_id))
                {
                    pane_obj.set_role(Some(role.clone()));
                }
            }
            // #210: 旧 TAKO_PANE_ID → 新 pane ID のマッピングを記録
            // （既存 claude CLI が旧 ID で MCP を呼んだとき dispatch で解決する）
            if let Some(old_id) =
                tako_core::tmux_backend::session_env(&socket, name, "TAKO_PANE_ID")
                    .and_then(|v| v.parse::<u64>().ok())
            {
                let old_pane = PaneId::from_raw(old_id);
                if old_pane != pane_id {
                    self.stale_pane_map.insert(old_pane, pane_id);
                }
            }
            // TAKO_PANE_ID / TAKO_TAB_ID を新 pane ID に更新
            tako_core::tmux_backend::set_session_env(
                &socket,
                name,
                "TAKO_PANE_ID",
                &pane_id.as_u64().to_string(),
            );
            tako_core::tmux_backend::set_session_env(
                &socket,
                name,
                "TAKO_TAB_ID",
                &tab_id.as_u64().to_string(),
            );
            recovered.push(name.clone());
        }
        if recovered.is_empty() {
            if let Some(tab_id) = self.workspace.find_tab_of_pane(first_id) {
                self.remove_tab_with(tab_id, CloseReason::Explicit, cx);
            }
        } else {
            self.workspace.active_tab_mut().tree_mut().equalize();
        }
        recovered
    }

    fn cleanup_orphan_tmux_with(&self, min_idle_secs: Option<u64>) -> Vec<String> {
        // セカンダリモードは backend_sessions（= protected）が空でプライマリの
        // セッションを全部 orphan と誤認するため、判定自体を行わない（Issue #113）
        if self.secondary || !self.tmux_persist || !tako_core::tmux_backend::available() {
            return Vec::new();
        }
        if tako_core::ports::other_tako_running() {
            eprintln!("info: 別の tako プロセスが動作中のため orphan クリーンアップをスキップ");
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
        tako_core::tmux_backend::cleanup_orphans(&socket, &protected, min_idle_secs)
    }

    /// レイアウトの保存（Phase 5.5 / FR-5）。構造が変わったときだけ書き込む。
    /// 定期ループ（2 秒）・dispatch 後・終了時に呼ばれる。セルフテスト中は
    /// ユーザーのレイアウトファイルを汚さない。
    /// tmux 不在でも保存する（Issue #30: その場合 session は None になり、
    /// 復元時は保存 cwd で新シェルを開く「構造のみ永続化」として機能する）。
    /// セカンダリモードは書かない（プライマリの layout.json を汚さない。Issue #113。
    /// tmux_persist=false で実質届かないが、persist トグル等の経路変更に耐える明示ガード）
    fn save_layout(&mut self) {
        if self.secondary || !self.tmux_persist || std::env::var_os("TAKO_SELF_TEST").is_some() {
            return;
        }
        // Issue #168: 2 秒ポーリング + dispatch 毎に呼ばれる。capture + 変化検出 +
        // （変化時のみ）ディスク書き込みのメインスレッド専有を計測
        let _span = tako_control::diag::perf_span("save_layout");
        let backend_sessions = &self.backend_sessions;
        let claude_resume_sessions = &self.claude_resume_sessions;
        let terminals = &self.terminals;
        let previews = &self.previews;
        let webviews = &self.webviews;
        // ペインログの取り込み位置（Issue #112 B。再起動後の差分取り込み基準として保存）
        let pane_log_history: HashMap<u64, u64> = self
            .pane_logs_lock()
            .all_logged_history()
            .into_iter()
            .map(|(pane, h)| (pane, h as u64))
            .collect();
        let mut layout = tako_control::layout::capture(
            &self.workspace,
            &|pane| tako_control::layout::PaneMeta {
                session: backend_sessions.get(&pane).cloned(),
                cwd: terminals
                    .get(&pane)
                    .and_then(|s| s.cwd())
                    .map(|p| p.display().to_string()),
                claude_session_id: claude_resume_sessions.get(&pane).cloned(),
                logged_history: pane_log_history.get(&pane.as_u64()).copied(),
                preview: previews
                    .get(&pane)
                    .map(|p| tako_control::layout::PreviewLayout {
                        path: p.path.display().to_string(),
                        mode: p.mode.to_wire().as_str().to_string(),
                    }),
                webview: webviews
                    .iter()
                    .find(|e| e.pane == Some(pane))
                    .map(|e| e.current_url()),
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
        // Web ビュー dock の退避分（#155）。表示分は PaneMeta.webview で tree に載る。
        // まだ開き直していない復元待ち（初回 render 前の保存）も失わずに引き継ぐ
        layout.webview_dock = self
            .webviews
            .iter()
            .filter(|e| e.pane.is_none())
            .map(|e| e.current_url())
            .chain(
                self.pending_webview_restore
                    .iter()
                    .filter(|(pane, _)| pane.is_none())
                    .map(|(_, url)| url.clone()),
            )
            .collect();
        let Ok(json) = serde_json::to_string(&layout) else {
            return;
        };
        if self.last_saved_layout.as_deref() == Some(json.as_str()) {
            return;
        }
        match tako_control::layout::save(&layout) {
            Ok(_) => self.last_saved_layout = Some(json),
            // 保存失敗は復元不能に直結するので診断ログにも残す（Issue #30）
            Err(e) => persist_diag(&format!("保存失敗: {e}")),
        }
    }

    /// 1 回の `claude agents --json` 成功結果を現在の backend ペインへ反映する。
    /// 成功結果に存在しないペインは Claude が終了済みなので関連を外し、次回 PC 起動で
    /// 古い会話を勝手に resume しない。スキャン自体が失敗した場合は呼ばれない。
    fn apply_claude_resume_sessions(&mut self, by_backend: &HashMap<String, String>) {
        self.claude_resume_sessions = self
            .backend_sessions
            .iter()
            .filter_map(|(pane, backend)| {
                by_backend
                    .get(backend)
                    .filter(|id| tako_control::transcript::is_valid_session_id(id))
                    .map(|id| (*pane, id.clone()))
            })
            .collect();
    }

    /// ペインログ（Issue #112 B）のロック（毒化耐性: 追記状態の破損より継続を優先）
    fn pane_logs_lock(&self) -> std::sync::MutexGuard<'_, tako_core::pane_log::PaneLogManager> {
        self.pane_logs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// ペインのログ命名メタ（タブ ID + role / title 由来のラベル）。
    /// タブツリーに無ければバックグラウンド（退避中）の由来タブ情報から引く
    fn pane_log_meta(&self, pane: PaneId) -> tako_core::pane_log::PaneLogMeta {
        for tab in self.workspace.tabs() {
            if let Some(p) = tab.tree().panes().iter().find(|p| p.id() == pane) {
                return tako_core::pane_log::PaneLogMeta {
                    tab: tab.id().as_u64(),
                    label: p.role().or(p.title()).map(str::to_string),
                };
            }
        }
        if let Some(bp) = self
            .workspace
            .shelved_panes()
            .iter()
            .find(|b| b.pane().id() == pane)
        {
            return tako_core::pane_log::PaneLogMeta {
                tab: bp.origin_tab().as_u64(),
                label: bp.pane().role().or(bp.pane().title()).map(str::to_string),
            };
        }
        tako_core::pane_log::PaneLogMeta::default()
    }

    /// ペインログの 1 tick 分の走査（Issue #112 B。2 秒ポーリングから呼ばれる）。
    /// 直接ペインは Term のメモリ読みでここで取り込み、tmux バックエンドペインは
    /// capture（サブプロセス）が要るため background 用のジョブとして返す
    fn collect_pane_log_work(&self) -> Vec<PaneLogJob> {
        if self.secondary {
            return Vec::new();
        }
        use tako_core::pane_log::{ChunkKind, PaneObservation, CAPTURE_CHUNK};
        let mut jobs = Vec::new();
        let mut mgr = self.pane_logs_lock();
        for (pane_id, session) in &self.terminals {
            // TmuxOpen ビューペイン（外部セッションの attach クライアント）は対象外
            if self.tmux_view_panes.contains_key(pane_id) {
                continue;
            }
            let meta = self.pane_log_meta(*pane_id);
            if let Some(backend) = self.backend_sessions.get(pane_id) {
                let (last_history, last_bytes) =
                    mgr.scan_baseline(pane_id.as_u64()).unwrap_or((0, 0));
                jobs.push(PaneLogJob {
                    pane: *pane_id,
                    session: backend.clone(),
                    meta,
                    last_history,
                    last_bytes,
                });
                continue;
            }
            // 直接ペイン: alacritty history の増分をメモリ読みで取り込む
            let history = session.history_size();
            let limit = session.scrollback_limit();
            let alt = session.is_alt_screen();
            let (last, _) = mgr.scan_baseline(pane_id.as_u64()).unwrap_or((0, 0));
            let chunk = if history < last {
                ChunkKind::None
            } else {
                let delta = history - last;
                if delta > 0 {
                    ChunkKind::Counted {
                        lines: session.history_plain_lines(0, delta.min(CAPTURE_CHUNK)),
                        delta,
                    }
                } else if history >= limit && !alt {
                    // 履歴が保持上限で飽和するとカウンタが増えない。末尾チャンクを
                    // 取り込み済み tail と照合して新規行だけ追記する
                    ChunkKind::Overlap {
                        captured: session.history_plain_lines(0, CAPTURE_CHUNK),
                    }
                } else {
                    ChunkKind::None
                }
            };
            mgr.apply(
                pane_id.as_u64(),
                &meta,
                PaneObservation {
                    history,
                    history_limit: limit,
                    bytes: 0,
                    alt_screen: alt,
                    chunk,
                },
            );
        }
        jobs
    }

    /// クローズ前のペインログフラッシュ素材（メタ + 直接ペインの履歴取りこぼし + 可視画面）。
    /// ペイン内容が空でログ状態も無い場合は None（空ログファイルを作らない）
    fn pane_log_close_data(&self, pane: PaneId) -> Option<PaneLogCloseData> {
        if self.secondary {
            return None;
        }
        let session = self.terminals.get(&pane)?;
        let visible = session.visible_lines();
        let has_state = self.pane_logs_lock().path_of(pane.as_u64()).is_some();
        if !has_state && !visible.iter().any(|l| !l.trim().is_empty()) {
            return None;
        }
        // 直接ペインは最終 tick 以降の履歴増分もここで拾い切る
        let catch_up = if self.backend_sessions.contains_key(&pane) {
            None
        } else {
            let history = session.history_size();
            let (last, _) = self
                .pane_logs_lock()
                .scan_baseline(pane.as_u64())
                .unwrap_or((0, 0));
            (history > last).then(|| {
                let delta = history - last;
                let take = delta.min(tako_core::pane_log::CAPTURE_CHUNK);
                (session.history_plain_lines(0, take), delta, history)
            })
        };
        Some(PaneLogCloseData {
            meta: self.pane_log_meta(pane),
            visible,
            catch_up,
        })
    }

    /// ペインログのクローズフラッシュ本体（`pane_log_close_data` と対）
    fn apply_pane_log_close(&self, pane: PaneId, data: PaneLogCloseData, reason: CloseReason) {
        use tako_core::pane_log::{ChunkKind, PaneObservation};
        let mut mgr = self.pane_logs_lock();
        if let Some((lines, delta, history)) = data.catch_up {
            mgr.apply(
                pane.as_u64(),
                &data.meta,
                PaneObservation {
                    history,
                    history_limit: usize::MAX,
                    bytes: 0,
                    alt_screen: false,
                    chunk: ChunkKind::Counted { lines, delta },
                },
            );
        }
        let reason_str = match reason {
            CloseReason::Explicit => "close",
            CloseReason::Exited => "exit",
        };
        mgr.flush_close(pane.as_u64(), &data.meta, &data.visible, reason_str);
    }

    /// カタログ同期用のペインメタ（Issue #112 A。backend セッション対応のあるペインのみ）
    fn collect_pane_meta_snapshots(&self) -> Vec<tako_control::sessions::PaneMetaSnapshot> {
        let logs = self.pane_logs_lock();
        let mut out = Vec::new();
        for (pane, backend) in &self.backend_sessions {
            let mut snap = tako_control::sessions::PaneMetaSnapshot {
                pane: pane.as_u64(),
                tmux_session: backend.clone(),
                ..Default::default()
            };
            for tab in self.workspace.tabs() {
                if let Some(p) = tab.tree().panes().iter().find(|p| p.id() == *pane) {
                    snap.tab = tab.id().as_u64();
                    snap.role = p.role().map(str::to_string);
                    snap.title = p.title().map(str::to_string);
                    break;
                }
            }
            if snap.tab == 0 {
                if let Some(bp) = self
                    .workspace
                    .shelved_panes()
                    .iter()
                    .find(|b| b.pane().id() == *pane)
                {
                    snap.tab = bp.origin_tab().as_u64();
                    snap.role = bp.pane().role().map(str::to_string);
                    snap.title = bp.pane().title().map(str::to_string);
                }
            }
            snap.cwd = self
                .terminals
                .get(pane)
                .and_then(|s| s.cwd())
                .map(|p| p.display().to_string());
            snap.log_file = logs.path_of(pane.as_u64()).map(|p| p.display().to_string());
            out.push(snap);
        }
        out
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
        let title = (self.workspace.tabs().len() + 1).to_string();
        let pane = Pane::new(PaneOrigin::User);
        let pane_id = pane.id();
        self.workspace.create_tab(title, pane);
        if let Err(e) = self.spawn_session(pane_id, SpawnOptions::default(), cx) {
            eprintln!("warning: タブを開けない: {e}");
            self.remove_pane(pane_id, cx); // 最後の 1 ペイン → タブごと畳まれる
        }
        self.sync_filetree_roots();
        cx.notify();
    }

    /// UI テーマのライト/ダーク反転（Issue #217。タブバーのテーマボタン用。
    /// dispatch::Theme と同じ状態遷移 + settings 永続化で UI / AI 操作の等価性を保つ）
    pub(crate) fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        use tako_core::theme::ThemeMode;
        let next = match self.theme.mode {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        };
        self.theme = Theme::for_mode(next);
        // セルフテスト中はユーザー設定を汚さない（dispatch 側と同方針）
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.theme = next.as_str().into();
            if let Err(e) = tako_control::settings::save(&settings) {
                eprintln!("warning: 設定を保存できない: {e}");
            }
        }
        cx.notify();
    }

    fn save_sidebar_width(&self) {
        if std::env::var_os("TAKO_SELF_TEST").is_some() {
            return;
        }
        let mut settings = tako_control::settings::load();
        settings.sidebar_width = self.sidebar_width as u32;
        if let Err(e) = tako_control::settings::save(&settings) {
            eprintln!("warning: 設定を保存できない: {e}");
        }
    }

    /// ⌘K コマンドパレットを開く（#217 カンプ。ペイン・コマンド検索）
    pub(crate) fn open_command_palette(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.command_palette = Some(CommandPalette {
            query: String::new(),
            selected: 0,
            mode: PaletteMode::Normal,
        });
        cx.notify();
    }

    fn open_ssh_palette(&mut self, cx: &mut Context<Self>) {
        let hosts = match tako_core::ssh_config::default_ssh_config_path() {
            Some(p) => tako_core::ssh_config::parse_ssh_config(&p),
            None => Vec::new(),
        };
        self.command_palette = Some(CommandPalette {
            query: String::new(),
            selected: 0,
            mode: PaletteMode::SshHost(hosts),
        });
        cx.notify();
    }

    fn open_recent_palette(&mut self, cx: &mut Context<Self>) {
        let recent = tako_core::recent::RecentList::load();
        self.command_palette = Some(CommandPalette {
            query: String::new(),
            selected: 0,
            mode: PaletteMode::RecentItems(recent.entries),
        });
        cx.notify();
    }

    /// コマンドパレットの候補（#217。query の部分一致で絞り込み済み）
    fn palette_items(&self, query: &str) -> Vec<PaletteItem> {
        let q = query.to_lowercase();

        // SSH ホスト選択モード
        if let Some(ref palette) = self.command_palette {
            match &palette.mode {
                PaletteMode::SshHost(hosts) => {
                    let items: Vec<PaletteItem> = hosts
                        .iter()
                        .map(|h| PaletteItem::SshHost(h.clone()))
                        .collect();
                    return if q.is_empty() {
                        items
                    } else {
                        items
                            .into_iter()
                            .filter(|item| item.label().to_lowercase().contains(&q))
                            .collect()
                    };
                }
                PaletteMode::RecentItems(entries) => {
                    let items: Vec<PaletteItem> = entries
                        .iter()
                        .map(|e| PaletteItem::Recent(e.clone()))
                        .collect();
                    return if q.is_empty() {
                        items
                    } else {
                        items
                            .into_iter()
                            .filter(|item| item.label().to_lowercase().contains(&q))
                            .collect()
                    };
                }
                PaletteMode::Normal => {}
            }
        }

        let mut items: Vec<PaletteItem> = Vec::new();
        // ペイン（全タブ）
        for tab in self.workspace.tabs() {
            for pane in tab.tree().panes() {
                let name = pane
                    .title()
                    .or_else(|| pane.role())
                    .map(str::to_string)
                    .or_else(|| {
                        self.terminals
                            .get(&pane.id())
                            .and_then(|s| s.title())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "ターミナル".into());
                let state = self
                    .terminals
                    .get(&pane.id())
                    .map(|s| match s.command_state() {
                        tako_core::CommandState::Failed(_) => "failed",
                        tako_core::CommandState::Running => "running",
                        tako_core::CommandState::Idle => "idle",
                        tako_core::CommandState::Unknown => "",
                    })
                    .unwrap_or("");
                items.push(PaletteItem::Pane(
                    pane.id(),
                    tab.title().to_string(),
                    format!(
                        "{name}{}",
                        if state.is_empty() {
                            String::new()
                        } else {
                            format!(" \u{00B7} {state}")
                        }
                    ),
                ));
            }
        }
        // 固定コマンド
        const COMMANDS: &[(&str, &str)] = &[
            ("新しいタブ", "new-tab"),
            ("テーマをライト/ダーク切替", "toggle-theme"),
            ("ファイルツリーを開閉", "toggle-files"),
            ("バックグラウンドドロワーを開閉", "toggle-drawer"),
            ("fleet パネルを開く", "panel-fleet"),
            ("orch パネルを開く", "panel-orch"),
            ("git パネルを開く", "panel-git"),
            ("ペインを右に分割", "split-right"),
            ("ペインを下に分割", "split-down"),
        ];
        for (label, id) in COMMANDS {
            items.push(PaletteItem::Command(label, id));
        }
        if q.is_empty() {
            return items;
        }
        items
            .into_iter()
            .filter(|item| item.label().to_lowercase().contains(&q))
            .collect()
    }

    /// コマンドパレットのキー入力処理（#217）。true = 消費した
    fn handle_palette_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(palette) = self.command_palette.as_mut() else {
            return false;
        };
        match keystroke.key.as_str() {
            "escape" => {
                self.command_palette = None;
            }
            "enter" => {
                let query = palette.query.clone();
                let selected = palette.selected;
                let items = self.palette_items(&query);
                self.command_palette = None;
                if let Some(item) = items.into_iter().nth(selected) {
                    self.palette_execute(item, cx);
                }
            }
            "up" => {
                palette.selected = palette.selected.saturating_sub(1);
            }
            "down" => {
                palette.selected = palette.selected.saturating_add(1);
            }
            "backspace" => {
                palette.query.pop();
                palette.selected = 0;
            }
            _ => {
                if let Some(ch) = keystroke.key_char.as_deref() {
                    if !ch.chars().any(|c| c.is_control()) {
                        palette.query.push_str(ch);
                        palette.selected = 0;
                    }
                }
            }
        }
        cx.notify();
        true
    }

    /// コマンドパレットの実行（#217）
    fn palette_execute(&mut self, item: PaletteItem, cx: &mut Context<Self>) {
        match item {
            PaletteItem::Pane(pane, _, _) => self.jump_to_pane(pane, cx),
            PaletteItem::Command(_, id) => match id {
                "new-tab" => self.new_tab(cx),
                "toggle-theme" => self.toggle_theme(cx),
                "toggle-files" => {
                    self.toggle_filetree();
                    cx.notify();
                }
                "toggle-drawer" => {
                    self.drawer_visible = !self.drawer_visible;
                    cx.notify();
                }
                "panel-fleet" => self.toggle_panel_view(PanelView::Tmux, cx),
                "panel-orch" => self.toggle_panel_view(PanelView::Orch, cx),
                "panel-git" => self.toggle_panel_view(PanelView::Git, cx),
                "split-right" => self.split(SplitDirection::Right, cx),
                "split-down" => self.split(SplitDirection::Down, cx),
                _ => {}
            },
            PaletteItem::SshHost(host) => {
                self.open_ssh_host(host, cx);
            }
            PaletteItem::Recent(entry) => {
                self.open_recent_entry(entry, cx);
            }
        }
    }

    fn open_ssh_host(&mut self, host: tako_core::ssh_config::SshHost, cx: &mut Context<Self>) {
        let cmd = host.ssh_command();
        let tab_title = format!("ssh:{}", host.name);
        let pane = tako_core::Pane::new(tako_core::PaneOrigin::User);
        let pane_id = pane.id();
        let tab_id = self.workspace.create_tab(tab_title.clone(), pane);
        if let Some(tab) = self.workspace.get_tab_mut(tab_id) {
            tab.set_title_manual(tab_title);
        }
        self.attach_session(
            pane_id,
            tako_core::SpawnOptions {
                command: Some(tako_core::SpawnCommand {
                    program: cmd[0].clone(),
                    args: cmd[1..].to_vec(),
                }),
                ..Default::default()
            },
        );
        self.scroll_active_tab_into_view();

        let mut recent = tako_core::recent::RecentList::load();
        recent.push(tako_core::recent::RecentEntry::Ssh { host: host.name });
        recent.save();

        cx.notify();
    }

    fn open_recent_entry(&mut self, entry: tako_core::recent::RecentEntry, cx: &mut Context<Self>) {
        match entry {
            tako_core::recent::RecentEntry::Directory { ref path } => {
                let dir = std::path::PathBuf::from(path);
                if dir.is_dir() {
                    self.open_dir_in_new_tab(
                        dir,
                        tako_core::recent::RecentEntry::Directory {
                            path: String::new(),
                        },
                        cx,
                    );
                }
            }
            tako_core::recent::RecentEntry::Repository { ref path } => {
                let dir = std::path::PathBuf::from(path);
                if dir.is_dir() {
                    self.open_dir_in_new_tab(
                        dir,
                        tako_core::recent::RecentEntry::Repository {
                            path: String::new(),
                        },
                        cx,
                    );
                }
            }
            tako_core::recent::RecentEntry::Ssh { ref host } => {
                let hosts = match tako_core::ssh_config::default_ssh_config_path() {
                    Some(p) => tako_core::ssh_config::parse_ssh_config(&p),
                    None => Vec::new(),
                };
                let ssh_host = hosts.into_iter().find(|h| h.name == *host).unwrap_or(
                    tako_core::ssh_config::SshHost {
                        name: host.clone(),
                        hostname: None,
                        user: None,
                        port: None,
                    },
                );
                self.open_ssh_host(ssh_host, cx);
            }
        }
    }

    /// 失敗遷移を検知して Attention トーストを積む（#217 カンプ。periodic から呼ぶ）
    fn update_attention_toasts(&mut self) {
        let current: std::collections::HashSet<PaneId> = self
            .terminals
            .iter()
            .filter(|(_, s)| matches!(s.command_state(), tako_core::CommandState::Failed(_)))
            .map(|(id, _)| *id)
            .collect();
        for pane_id in current.difference(&self.known_failed) {
            let exit_code = self
                .terminals
                .get(pane_id)
                .and_then(|s| match s.command_state() {
                    tako_core::CommandState::Failed(code) => Some(code),
                    _ => None,
                })
                .unwrap_or(1);
            // ペイン名とタブ情報
            let mut name = "ターミナル".to_string();
            let mut tab_title = String::new();
            let mut pane_index = 0;
            for tab in self.workspace.tabs() {
                let panes = tab.tree().panes();
                if let Some(pos) = panes.iter().position(|p| p.id() == *pane_id) {
                    let p = &panes[pos];
                    name = p
                        .title()
                        .or_else(|| p.role())
                        .map(str::to_string)
                        .unwrap_or(name);
                    tab_title = tab.title().to_string();
                    pane_index = pos + 1;
                    break;
                }
            }
            self.toasts.push(AttentionToast {
                pane: *pane_id,
                title: format!("{} が失敗", truncate(&name, 24)),
                detail: format!("{tab_title} \u{203A} pane {pane_index} \u{00B7} exit {exit_code}"),
                at: std::time::Instant::now(),
            });
            // 溜まりすぎ防止（最新 3 件のみ表示対象）
            if self.toasts.len() > 3 {
                self.toasts.remove(0);
            }
        }
        self.known_failed = current;
    }

    fn open_directory(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(dir) = paths.into_iter().next() {
                    let _ = this.update(cx, |app: &mut TakoApp, cx| {
                        app.open_dir_in_new_tab(
                            dir,
                            tako_core::recent::RecentEntry::Directory {
                                path: String::new(),
                            },
                            cx,
                        );
                    });
                }
            }
        })
        .detach();
    }

    /// サイドバー「+」ボタン: フォルダピッカー → 現タブにルート追加（#268）
    fn add_tree_root(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(dir) = paths.into_iter().next() {
                    let _ = this.update(cx, |app: &mut TakoApp, cx| {
                        let dir = dir.canonicalize().unwrap_or(dir);
                        let tab_id = app.workspace.active_tab().id();
                        if let Some(tab) = app.workspace.get_tab_mut(tab_id) {
                            tab.add_pinned_folder(dir);
                        }
                        app.sync_filetree_roots();
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn open_repository(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(dir) = paths.into_iter().next() {
                    let git_root = find_git_root(&dir).unwrap_or(dir);
                    let _ = this.update(cx, |app: &mut TakoApp, cx| {
                        app.open_dir_in_new_tab(
                            git_root,
                            tako_core::recent::RecentEntry::Repository {
                                path: String::new(),
                            },
                            cx,
                        );
                    });
                }
            }
        })
        .detach();
    }

    fn open_dir_in_new_tab(
        &mut self,
        dir: std::path::PathBuf,
        entry_kind: tako_core::recent::RecentEntry,
        cx: &mut Context<Self>,
    ) {
        use tako_core::recent::{RecentEntry, RecentList};

        let dir = dir.canonicalize().unwrap_or(dir);
        let label = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.display().to_string());

        let pane = tako_core::Pane::new(tako_core::PaneOrigin::User);
        let pane_id = pane.id();
        let tab_id = self.workspace.create_tab(label, pane);
        self.attach_session(
            pane_id,
            tako_core::SpawnOptions {
                cwd: Some(dir.clone()),
                ..Default::default()
            },
        );

        // ファイルツリーにフォルダを追加
        if let Some(tab) = self.workspace.get_tab_mut(tab_id) {
            tab.add_pinned_folder(dir.clone());
        }
        self.sync_filetree_roots();
        self.scroll_active_tab_into_view();

        // Recent に記録
        let path_str = dir.to_string_lossy().to_string();
        let recent_entry = match entry_kind {
            RecentEntry::Repository { .. } => RecentEntry::Repository { path: path_str },
            _ => RecentEntry::Directory { path: path_str },
        };
        let mut recent = RecentList::load();
        recent.push(recent_entry);
        recent.save();

        cx.notify();
    }

    fn activate_tab_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.workspace.tabs().get(index).map(|t| t.id()) {
            let _ = self.workspace.activate_tab(id);
        }
        self.scroll_active_tab_into_view();
        self.sync_filetree_roots();
        cx.notify();
    }

    fn select_all_preview(&mut self, cx: &mut Context<Self>) {
        let pane_id = self.focused_pane();
        if let Some(edit) = self.preview_edits.get_mut(&pane_id) {
            if edit.editing {
                edit.buffer.select_all();
                self.sync_preview_selection_from_editor(pane_id);
                cx.notify();
                return;
            }
        }
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
        let texts = self.preview_line_texts.get(&pane_id)?;

        // Code / Markdown は StyledText が実際に使った shaping 結果を逆写像する。
        // 旧実装の `x / terminal_cell_width` は、テーマフォントの実 advance、太字、
        // Markdown 見出しの font-size、タブ / 日本語を無視するため、右へ行くほど選択がずれた。
        if let Some(layouts) = self
            .preview_text_layouts
            .get(&pane_id)
            .filter(|layouts| layouts.iter().any(Option::is_some))
        {
            return preview_text_layout_hit_test(layouts, texts, position);
        }

        if let (Some(line_bounds), Some(char_bounds)) = (
            self.preview_line_bounds.get(&pane_id),
            self.preview_pdf_char_bounds.get(&pane_id),
        ) {
            return preview_render::pdf_text_hit_test(line_bounds, char_bounds, texts, position);
        }
        texts.last().map(|last| (texts.len() - 1, last.len()))
    }

    /// PDF リンクのヒットテスト（#271 / #315）。マウス位置にある PDF リンクのインデックスを返す。
    /// ページ画像の bounds は canvas paint 時に直接記録したものを使う（テキストレイヤ不要）。
    fn pdf_link_at_position(&self, pane_id: PaneId, position: Point<Pixels>) -> Option<usize> {
        let state = self.previews.get(&pane_id)?;
        let data = match &state.content {
            preview::PreviewContent::Pdf(data) => data,
            _ => return None,
        };
        if data.links.is_empty() {
            return None;
        }
        let page_bounds_map = self.preview_pdf_page_image_bounds.get(&pane_id)?;

        for (link_idx, link) in data.links.links.iter().enumerate() {
            let Some(page_size) = data.page_sizes.get(link.page_index) else {
                continue;
            };
            let Some(image_bounds) = page_bounds_map.get(&link.page_index) else {
                continue;
            };
            let screen_bounds =
                preview_render::pdf_box_to_screen(link.bbox, *page_size, *image_bounds);
            if screen_bounds.contains(&position) {
                return Some(link_idx);
            }
        }
        None
    }

    /// PDF プレビューのリンクホバー状態を更新する（#271 / #315）。
    /// すべてのレンダリング済み PDF ペインをチェックする（focused_pane に依存しない）。
    fn update_pdf_link_hover(
        &mut self,
        position: Point<Pixels>,
        cmd_held: bool,
        cx: &mut Context<Self>,
    ) {
        let old = self.preview_pdf_hovered_link;
        if !cmd_held {
            if old.is_some() {
                self.preview_pdf_hovered_link = None;
                cx.notify();
            }
            return;
        }
        let pane_ids: Vec<PaneId> = self.preview_pdf_page_image_bounds.keys().copied().collect();
        let mut found = None;
        for pane_id in pane_ids {
            if let Some(idx) = self.pdf_link_at_position(pane_id, position) {
                found = Some((pane_id, idx));
                break;
            }
        }
        if found != old {
            self.preview_pdf_hovered_link = found;
            cx.notify();
        }
    }

    /// PDF リンクをフォローする（#271）。外部 URL はブラウザ、内部リンクはページジャンプ。
    fn follow_pdf_link(&mut self, pane_id: PaneId, link_idx: usize, cx: &mut Context<Self>) {
        let state = match self.previews.get(&pane_id) {
            Some(s) => s,
            None => return,
        };
        let data = match &state.content {
            preview::PreviewContent::Pdf(data) => data,
            _ => return,
        };
        let link = match data.links.links.get(link_idx) {
            Some(l) => l,
            None => return,
        };
        match &link.target {
            tako_core::PdfLinkTarget::Url { url } => {
                let _ = std::process::Command::new("open").arg(url).spawn();
            }
            tako_core::PdfLinkTarget::Page { page } => {
                if let Err(e) = <TakoApp as PreviewHost>::update_preview_view(
                    self,
                    pane_id,
                    tako_core::PreviewViewUpdate {
                        page: Some(*page),
                        ..tako_core::PreviewViewUpdate::default()
                    },
                ) {
                    eprintln!("warning: PDF ページジャンプ失敗: {e}");
                }
            }
        }
        cx.notify();
    }

    fn refresh_preview_from_editor(&mut self, pane_id: PaneId) {
        let (previews, edits) = (&mut self.previews, &self.preview_edits);
        if let (Some(state), Some(edit)) = (previews.get_mut(&pane_id), edits.get(&pane_id)) {
            preview::apply_editor_text(state, edit);
        }
        self.sync_preview_selection_from_editor(pane_id);
    }

    fn sync_preview_selection_from_editor(&mut self, pane_id: PaneId) {
        let Some(edit) = self.preview_edits.get(&pane_id) else {
            return;
        };
        let anchor = edit.buffer.anchor().unwrap_or(edit.buffer.cursor());
        let head = edit.buffer.cursor();
        self.preview_selections.insert(
            pane_id,
            PreviewSelection {
                anchor: edit.buffer.line_byte_col(anchor),
                head: edit.buffer.line_byte_col(head),
            },
        );
    }

    fn sync_editor_selection_from_preview(&mut self, pane_id: PaneId) {
        let Some(selection) = self.preview_selections.get(&pane_id).cloned() else {
            return;
        };
        let Some(edit) = self.preview_edits.get_mut(&pane_id) else {
            return;
        };
        let anchor = edit
            .buffer
            .offset_for_line_byte_col(selection.anchor.0, selection.anchor.1);
        let head = edit
            .buffer
            .offset_for_line_byte_col(selection.head.0, selection.head.1);
        edit.buffer.set_cursor(anchor, false);
        edit.buffer.set_cursor(head, true);
    }

    fn set_preview_editing_local(&mut self, pane_id: PaneId, enabled: bool) -> Result<(), String> {
        if !self.previews.contains_key(&pane_id) {
            return Err("プレビューペインではない".into());
        }
        if enabled && !self.preview_edits.contains_key(&pane_id) {
            let state = self.previews.get(&pane_id).expect("上で確認済み");
            self.preview_edits
                .insert(pane_id, preview::EditState::open(state)?);
        }
        if let Some(edit) = self.preview_edits.get_mut(&pane_id) {
            edit.editing = enabled;
            edit.message = None;
        }
        if enabled {
            self.refresh_preview_from_editor(pane_id);
        } else if self
            .preview_edits
            .get(&pane_id)
            .is_some_and(|edit| !edit.dirty())
        {
            self.preview_edits.remove(&pane_id);
        }
        Ok(())
    }

    fn apply_preview_text_local(&mut self, pane_id: PaneId, text: String) -> Result<(), String> {
        self.set_preview_editing_local(pane_id, true)?;
        let edit = self.preview_edits.get_mut(&pane_id).expect("編集開始済み");
        edit.buffer.set_text(text);
        edit.message = None;
        self.refresh_preview_from_editor(pane_id);
        Ok(())
    }

    fn save_preview_local(&mut self, pane_id: PaneId) -> Result<(), String> {
        let edit = self
            .preview_edits
            .get_mut(&pane_id)
            .ok_or_else(|| "編集モードを開始していない".to_string())?;
        match edit.buffer.save() {
            Ok(()) => {
                edit.message = Some("保存しました".into());
                self.refresh_preview_from_editor(pane_id);
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                edit.message = Some(message.clone());
                Err(message)
            }
        }
    }

    fn save_focused_preview(&mut self, cx: &mut Context<Self>) {
        let pane_id = self.focused_pane();
        let _ = self.save_preview_local(pane_id);
        cx.notify();
    }

    fn preview_undo_local(&mut self, pane_id: PaneId) -> Result<bool, String> {
        let edit = self
            .preview_edits
            .get_mut(&pane_id)
            .ok_or_else(|| "編集モードを開始していない".to_string())?;
        let undone = edit.buffer.undo();
        if undone {
            edit.message = None;
            edit.save_status = None;
            self.refresh_preview_from_editor(pane_id);
        }
        Ok(undone)
    }

    fn preview_redo_local(&mut self, pane_id: PaneId) -> Result<bool, String> {
        let edit = self
            .preview_edits
            .get_mut(&pane_id)
            .ok_or_else(|| "編集モードを開始していない".to_string())?;
        let redone = edit.buffer.redo();
        if redone {
            edit.message = None;
            edit.save_status = None;
            self.refresh_preview_from_editor(pane_id);
        }
        Ok(redone)
    }

    fn preview_search_local(
        &mut self,
        pane_id: PaneId,
        query: Option<String>,
        direction: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        // 閲覧中でも検索可能にするため、編集セッションが無ければバッファを開く
        if !self.preview_edits.contains_key(&pane_id) && self.previews.contains_key(&pane_id) {
            let state = self.previews.get(&pane_id).unwrap();
            if let Ok(mut new_edit) = preview::EditState::open(state) {
                new_edit.editing = false;
                self.preview_edits.insert(pane_id, new_edit);
            }
        }
        let edit = self
            .preview_edits
            .get_mut(&pane_id)
            .ok_or_else(|| "プレビューペインではない".to_string())?;
        if let Some(q) = query {
            edit.search_query = q;
            edit.search_hits = edit.buffer.find_all(&edit.search_query);
            edit.search_index = 0;
        }
        match direction.unwrap_or("next") {
            "prev" => {
                if let Some(hit) = edit
                    .buffer
                    .find_prev(&edit.search_query, edit.buffer.cursor())
                {
                    edit.buffer.set_cursor(hit.start, false);
                    edit.search_index = edit
                        .search_hits
                        .iter()
                        .position(|h| h.start == hit.start)
                        .unwrap_or(0);
                }
            }
            _ => {
                let from = if edit.search_hits.is_empty() {
                    0
                } else {
                    edit.buffer.cursor()
                        + edit.buffer.text()[edit.buffer.cursor()..]
                            .chars()
                            .next()
                            .map(char::len_utf8)
                            .unwrap_or(0)
                };
                if let Some(hit) = edit.buffer.find_next(&edit.search_query, from) {
                    edit.buffer.set_cursor(hit.start, false);
                    edit.search_index = edit
                        .search_hits
                        .iter()
                        .position(|h| h.start == hit.start)
                        .unwrap_or(0);
                }
            }
        }
        let total = edit.search_hits.len();
        let index = if total > 0 { edit.search_index + 1 } else { 0 };
        Ok(serde_json::json!({
            "query": edit.search_query,
            "total": total,
            "index": index,
        }))
    }

    fn preview_replace_local(
        &mut self,
        pane_id: PaneId,
        query: &str,
        replacement: &str,
        all: bool,
    ) -> Result<serde_json::Value, String> {
        let edit = self
            .preview_edits
            .get_mut(&pane_id)
            .ok_or_else(|| "編集モードを開始していない".to_string())?;
        if !edit.editing {
            return Err("編集モードが無効".into());
        }
        if all {
            let count = edit.buffer.replace_all(query, replacement);
            edit.search_hits = edit.buffer.find_all(&edit.search_query);
            self.refresh_preview_from_editor(pane_id);
            self.schedule_autosave(pane_id);
            Ok(serde_json::json!({ "replaced": count }))
        } else {
            let hits = edit.buffer.find_all(query);
            if let Some(hit) = hits.into_iter().find(|h| h.start >= edit.buffer.cursor()) {
                edit.buffer.replace_range(hit.start..hit.end, replacement);
                edit.search_hits = edit.buffer.find_all(&edit.search_query);
                self.refresh_preview_from_editor(pane_id);
                self.schedule_autosave(pane_id);
                Ok(serde_json::json!({ "replaced": 1 }))
            } else if let Some(hit) = edit.buffer.find_all(query).into_iter().next() {
                edit.buffer.replace_range(hit.start..hit.end, replacement);
                edit.search_hits = edit.buffer.find_all(&edit.search_query);
                self.refresh_preview_from_editor(pane_id);
                self.schedule_autosave(pane_id);
                Ok(serde_json::json!({ "replaced": 1 }))
            } else {
                Ok(serde_json::json!({ "replaced": 0 }))
            }
        }
    }

    fn schedule_autosave(&mut self, pane_id: PaneId) {
        if !self
            .preview_edits
            .get(&pane_id)
            .is_some_and(|edit| edit.autosave && edit.editing && edit.dirty())
        {
            return;
        }
        if self.autosave_pending.contains(&pane_id) {
            return;
        }
        self.autosave_pending.insert(pane_id);
    }

    fn start_autosave_timer(&self, pane_id: PaneId, cx: &mut Context<Self>) {
        if !self.autosave_pending.contains(&pane_id) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            this.update(cx, |this, cx| {
                this.run_autosave(pane_id);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_autosave(&mut self, pane_id: PaneId) {
        if !self.autosave_pending.remove(&pane_id) {
            return;
        }
        if !self
            .preview_edits
            .get(&pane_id)
            .is_some_and(|edit| edit.autosave && edit.editing && edit.dirty())
        {
            return;
        }
        match self.save_preview_local(pane_id) {
            Ok(()) => {
                if let Some(edit) = self.preview_edits.get_mut(&pane_id) {
                    edit.save_status = Some(preview::SaveStatus::Saved);
                    edit.message = None;
                }
            }
            Err(msg) => {
                if let Some(edit) = self.preview_edits.get_mut(&pane_id) {
                    if msg.contains("外部") {
                        edit.save_status = Some(preview::SaveStatus::Conflict);
                    } else {
                        edit.save_status = Some(preview::SaveStatus::Error(msg));
                    }
                }
            }
        }
    }

    fn handle_preview_edit_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        use tako_core::CursorMovement;

        let pane_id = self.focused_pane();

        // 検索バー表示中: キーを検索/置換フィールドにルーティング
        if self
            .preview_edits
            .get(&pane_id)
            .is_some_and(|edit| edit.search_visible)
        {
            if keystroke.modifiers.platform
                || keystroke.modifiers.control
                || keystroke.modifiers.alt
            {
                return false;
            }
            return self.handle_search_bar_key(pane_id, keystroke, cx);
        }

        if !self
            .preview_edits
            .get(&pane_id)
            .is_some_and(|edit| edit.editing)
        {
            return false;
        }
        if keystroke.modifiers.platform || keystroke.modifiers.control || keystroke.modifiers.alt {
            return false;
        }
        if keystroke.key == "escape" {
            let _ = self.set_preview_editing_local(pane_id, false);
            cx.notify();
            return true;
        }
        self.sync_editor_selection_from_preview(pane_id);
        let shift = keystroke.modifiers.shift;
        let Some(edit) = self.preview_edits.get_mut(&pane_id) else {
            return false;
        };
        let handled = match keystroke.key.as_str() {
            "backspace" => {
                edit.buffer.delete_backward();
                true
            }
            "delete" => {
                edit.buffer.delete_forward();
                true
            }
            "enter" => {
                edit.buffer.newline();
                true
            }
            "left" => {
                edit.buffer.move_cursor(CursorMovement::Left, shift);
                true
            }
            "right" => {
                edit.buffer.move_cursor(CursorMovement::Right, shift);
                true
            }
            "up" => {
                edit.buffer.move_cursor(CursorMovement::Up, shift);
                true
            }
            "down" => {
                edit.buffer.move_cursor(CursorMovement::Down, shift);
                true
            }
            "home" => {
                edit.buffer.move_cursor(CursorMovement::LineStart, shift);
                true
            }
            "end" => {
                edit.buffer.move_cursor(CursorMovement::LineEnd, shift);
                true
            }
            _ => false,
        };
        let is_text_change = matches!(keystroke.key.as_str(), "backspace" | "delete" | "enter");
        if handled {
            edit.message = None;
            self.refresh_preview_from_editor(pane_id);
            if is_text_change {
                self.schedule_autosave(pane_id);
                self.start_autosave_timer(pane_id, cx);
            }
            cx.notify();
        }
        handled
    }

    fn handle_search_bar_key(
        &mut self,
        pane_id: PaneId,
        keystroke: &Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(edit) = self.preview_edits.get_mut(&pane_id) else {
            return false;
        };
        match keystroke.key.as_str() {
            "escape" => {
                edit.search_visible = false;
                cx.notify();
                true
            }
            "enter" => {
                let shift = keystroke.modifiers.shift;
                let do_replace =
                    edit.search_focus == preview::SearchFieldFocus::Replace && edit.editing;
                let q = edit.search_query.clone();
                let r = edit.replace_text.clone();
                if shift {
                    let _ = self.preview_search_local(pane_id, None, Some("prev"));
                } else if do_replace {
                    let _ = self.preview_replace_local(pane_id, &q, &r, false);
                } else {
                    let _ = self.preview_search_local(pane_id, None, Some("next"));
                }
                self.refresh_preview_from_editor(pane_id);
                cx.notify();
                true
            }
            "tab" => {
                edit.search_focus = match edit.search_focus {
                    preview::SearchFieldFocus::Query => preview::SearchFieldFocus::Replace,
                    preview::SearchFieldFocus::Replace => preview::SearchFieldFocus::Query,
                };
                cx.notify();
                true
            }
            "backspace" => {
                match edit.search_focus {
                    preview::SearchFieldFocus::Query => {
                        if edit.search_cursor > 0 {
                            let prev = edit.search_query[..edit.search_cursor]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            edit.search_query.drain(prev..edit.search_cursor);
                            edit.search_cursor = prev;
                            self.update_search_hits(pane_id);
                        }
                    }
                    preview::SearchFieldFocus::Replace => {
                        if edit.replace_cursor > 0 {
                            let prev = edit.replace_text[..edit.replace_cursor]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            edit.replace_text.drain(prev..edit.replace_cursor);
                            edit.replace_cursor = prev;
                        }
                    }
                }
                cx.notify();
                true
            }
            "delete" => {
                match edit.search_focus {
                    preview::SearchFieldFocus::Query => {
                        if edit.search_cursor < edit.search_query.len() {
                            let next = edit.search_cursor
                                + edit.search_query[edit.search_cursor..]
                                    .chars()
                                    .next()
                                    .map(char::len_utf8)
                                    .unwrap_or(0);
                            edit.search_query.drain(edit.search_cursor..next);
                            self.update_search_hits(pane_id);
                        }
                    }
                    preview::SearchFieldFocus::Replace => {
                        if edit.replace_cursor < edit.replace_text.len() {
                            let next = edit.replace_cursor
                                + edit.replace_text[edit.replace_cursor..]
                                    .chars()
                                    .next()
                                    .map(char::len_utf8)
                                    .unwrap_or(0);
                            edit.replace_text.drain(edit.replace_cursor..next);
                        }
                    }
                }
                cx.notify();
                true
            }
            "left" => {
                match edit.search_focus {
                    preview::SearchFieldFocus::Query => {
                        if edit.search_cursor > 0 {
                            edit.search_cursor = edit.search_query[..edit.search_cursor]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                        }
                    }
                    preview::SearchFieldFocus::Replace => {
                        if edit.replace_cursor > 0 {
                            edit.replace_cursor = edit.replace_text[..edit.replace_cursor]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                        }
                    }
                }
                cx.notify();
                true
            }
            "right" => {
                match edit.search_focus {
                    preview::SearchFieldFocus::Query => {
                        if edit.search_cursor < edit.search_query.len() {
                            edit.search_cursor += edit.search_query[edit.search_cursor..]
                                .chars()
                                .next()
                                .map(char::len_utf8)
                                .unwrap_or(0);
                        }
                    }
                    preview::SearchFieldFocus::Replace => {
                        if edit.replace_cursor < edit.replace_text.len() {
                            edit.replace_cursor += edit.replace_text[edit.replace_cursor..]
                                .chars()
                                .next()
                                .map(char::len_utf8)
                                .unwrap_or(0);
                        }
                    }
                }
                cx.notify();
                true
            }
            _ => false,
        }
    }

    fn insert_search_char(&mut self, pane_id: PaneId, text: &str) {
        let Some(edit) = self.preview_edits.get_mut(&pane_id) else {
            return;
        };
        if !edit.search_visible {
            return;
        }
        match edit.search_focus {
            preview::SearchFieldFocus::Query => {
                edit.search_query.insert_str(edit.search_cursor, text);
                edit.search_cursor += text.len();
                self.update_search_hits(pane_id);
            }
            preview::SearchFieldFocus::Replace => {
                edit.replace_text.insert_str(edit.replace_cursor, text);
                edit.replace_cursor += text.len();
            }
        }
    }

    fn update_search_hits(&mut self, pane_id: PaneId) {
        let Some(edit) = self.preview_edits.get_mut(&pane_id) else {
            return;
        };
        edit.search_hits = edit.buffer.find_all(&edit.search_query);
        edit.search_index = 0;
        if let Some(hit) = edit.buffer.find_next(&edit.search_query, 0) {
            edit.buffer.set_cursor(hit.start, false);
            self.refresh_preview_from_editor(pane_id);
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let pane_id = self.focused_pane();
        if self
            .preview_edits
            .get(&pane_id)
            .is_some_and(|edit| edit.editing)
        {
            self.sync_editor_selection_from_preview(pane_id);
            if let Some(edit) = self.preview_edits.get_mut(&pane_id) {
                edit.buffer.insert(&text);
                edit.message = None;
            }
            self.refresh_preview_from_editor(pane_id);
            self.schedule_autosave(pane_id);
            self.start_autosave_timer(pane_id, cx);
        } else if let Some(session) = self.focused_session() {
            session.paste(&text);
        }
        cx.notify();
    }

    // --- キー入力 ---

    fn handle_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        // Issue #168: キーストローク毎の処理コストを計測（入力レイテンシの内訳）
        let _span = tako_control::diag::perf_span("key_input");
        if self.pending_close_confirm.is_some() {
            match keystroke.key.as_str() {
                "enter" => self.close_confirm_accepted(cx),
                "escape" => self.close_confirm_cancelled(cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        // ⌘K コマンドパレット（#217）: 開いている間は全キーを消費する
        if self.command_palette.is_some() && self.handle_palette_key(keystroke, cx) {
            cx.stop_propagation();
            return;
        }
        if self.inline_edit.is_some() {
            self.handle_inline_edit_key(keystroke, cx);
            cx.stop_propagation();
            return;
        }

        if self.webview_dock_url_focused && self.handle_webview_dock_url_key(keystroke, cx) {
            cx.stop_propagation();
            return;
        }

        if self.handle_preview_edit_key(keystroke, cx) {
            cx.stop_propagation();
            return;
        }

        // フォーカスペインが動画プレビュー中ならキーボードショートカットを処理
        if self.handle_video_key(keystroke, cx) {
            cx.stop_propagation();
            return;
        }

        // cmd を含む未バインドのキーはシェルへ流さない
        if keystroke.modifiers.platform {
            return;
        }
        // 修飾付きキー（Shift+Enter 等）の CSI u 送出は**全ペインで**常時有効化する
        // （ModifiedOnly）。修飾付き Enter はレガシー形式だと素の \r に潰れて区別不能な
        // 一方、Claude Code は kitty protocol を要求・クエリせずとも CSI u 入力を解釈する
        // （2026-07-02 v2.1.198 素の PTY で実測）ため、内側アプリの要求は観測できなくても
        // CSI u で送るのが正しい。tmux バックエンドペインは extended-keys always +
        // extended-keys-format csi-u が解釈して内側へ届け、直接 spawn ペイン
        // （tmux 無し環境 = Homebrew 配布の既定）はそのまま届く。
        // 旧実装はバックエンドペイン限定だったため tmux 無し環境で Shift+Enter 改行が
        // 死んでいた（Issue #28 の根因）。CSI u 非対応アプリ（素の zsh 等）では修飾付き
        // Enter が「3;2u」風の文字列になるが、バックエンドペインは 2026-06-12 から
        // 同挙動で実害報告なし（受容済みトレードオフ）。
        // ただし **Esc 単押しは CSI 27u にしない**（ModifiedOnly）: tmux 3.6 は受信した
        // CSI 27u を内側ペインの kitty 要求の有無に関係なく素通しするため、CSI u
        // 非対応アプリ（素の zsh 等）の入力欄に「27u」が文字として挿入される
        // （2026-06-12 実機バグ）。素の \e は tmux が escape-time で正しく解釈し
        // 内側へ素のまま届く（core e2e で固定）。Esc 単押しの CSI 27u はアプリ自身が
        // kitty disambiguate を push 済み（= 確実に解釈できる）ときだけ（Full）
        let kitty_requested = self
            .focused_session()
            .map(|s| s.disambiguate_keys())
            .unwrap_or(false);
        let csi_u = if kitty_requested {
            CsiUMode::Full
        } else {
            CsiUMode::ModifiedOnly
        };
        if let Some(bytes) = keystroke_to_bytes(keystroke, csi_u) {
            // tmux スクロール中（copy-mode）は iTerm2 流に最下部へ戻してから流す
            // （copy-mode にキーが飲まれて「入力が反映されない」症状の根治）
            let pane = self.focused_pane();
            self.cancel_scroll_before_input(pane);
            let mut enter_baseline = None;
            let mut wrote = false;
            if let Some(session) = self.focused_session() {
                session.clear_selection();
                // Enter 送達検証の基準（Issue #95）: claude TUI の入力欄に非空テキストが
                // ある状態への Enter は busy 中などに取りこぼされることがある。
                // 書き込み前の入力欄内容を控え、残留していれば Enter を単独再送する
                if bytes.as_slice() == b"\r" && session.is_alt_screen() {
                    use tako_control::claude_tui;
                    let lines = session.visible_lines();
                    enter_baseline = claude_tui::input_line(&lines)
                        .filter(|s| !claude_tui::input_content_is_empty(s))
                        .map(str::to_string);
                }
                session.write(bytes);
                wrote = true;
            }
            if wrote {
                if let Some(baseline) = enter_baseline {
                    self.queue_enter_verify(pane, baseline);
                }
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
        if let Some(edit) = self.preview_edits.get(&pane).filter(|edit| edit.editing) {
            let (line, byte_col) = edit.buffer.line_byte_col(edit.buffer.cursor());
            let text = self.preview_line_texts.get(&pane)?.get(line)?;
            let byte_col = snap_to_char_boundary(text, byte_col.min(text.len()));
            let layout = self.preview_text_layouts.get(&pane)?.get(line)?.as_ref()?;
            return layout.position_for_index(byte_col);
        }
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane)?;
        let cell = self.cell_size_for_pane(pane)?;
        let session = self.terminals.get(&pane)?;
        let screen = session.screen(&self.theme);
        let (col, row) = screen.cursor?;
        let x = f32::from(cell.width) * col as f32;
        // サブラインスクロール中は描画シフトぶんカーソル位置も上へずれる（#159）
        let subline = session.scroll_subline_fract() * f32::from(cell.height);
        Some(point(
            area.origin.x + px(x),
            area.origin.y + cell.height * row as f32 - px(subline),
        ))
    }

    /// IME 候補ウィンドウ用のカーソル位置。CursorShape::Hidden でもビューポート内なら返す。
    /// カーソルが表示中なら通常カーソル位置、非表示でもビューポート内なら ime_cursor、
    /// どちらも無い（スクロールバック中）なら None
    fn pane_cursor_origin_for_ime(
        &self,
        pane: PaneId,
        window: &mut Window,
    ) -> Option<Point<Pixels>> {
        if let Some(origin) = self.pane_cursor_origin(pane, window) {
            return Some(origin);
        }
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane)?;
        let cell = self.cell_size_for_pane(pane)?;
        let session = self.terminals.get(&pane)?;
        let screen = session.screen(&self.theme);
        let (col, row) = screen.ime_cursor?;
        let x = f32::from(cell.width) * col as f32;
        let subline = session.scroll_subline_fract() * f32::from(cell.height);
        Some(point(
            area.origin.x + px(x),
            area.origin.y + cell.height * row as f32 - px(subline),
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
        window: &mut Window,
    ) -> Option<(usize, usize, bool)> {
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane_id)?;
        if !area.contains(&position) {
            return None;
        }
        self.cell_at_clamped(pane_id, position, window)
    }

    /// `cell_at` のクランプ版。テキスト領域外の座標も最寄りセルへ写像する。
    /// 選択ドラッグ中はペインを外れても選択を伸ばし続ける必要があるためこちらを使う
    fn cell_at_clamped(
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
        // サブラインスクロール中は描画が fract 行ぶん上へずれているため、
        // マウス座標も同じだけ補正して視覚位置とグリッド行を一致させる（#159）
        let subline = session.scroll_subline_fract() * f32::from(cell.height);
        let y = ((f32::from(local.y) + subline) / f32::from(cell.height)).max(0.0);
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
                    if m.ctx_percent.is_some() || m.usage_text.is_some() || m.limit_5h.is_some() {
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
        // usage トークン推移の履歴（#217 スパークライン。取れたときだけ・変化時だけ積む）
        if let Some(tok) = self
            .agent_metrics
            .usage_text
            .as_deref()
            .and_then(parse_tokens_value)
        {
            if self.usage_history.back().copied() != Some(tok) {
                self.usage_history.push_back(tok);
                if self.usage_history.len() > 5 {
                    self.usage_history.pop_front();
                }
            }
        }
    }

    fn update_sleep_guard(&mut self) {
        use tako_core::CommandState;
        let settings = tako_control::settings::load();
        // OSC 133 で Running が検知できているペイン数
        let running_count = self
            .terminals
            .values()
            .filter(|s| matches!(s.command_state(), CommandState::Running))
            .count();
        // persist 復元後に OSC 133 未検知（Unknown）だがバックエンドに
        // 実行中の子プロセスがいるペイン数（#324: 復元 worker の busy 漏れ根治）
        let unknown_backends: Vec<&str> = self
            .terminals
            .iter()
            .filter(|(_, s)| matches!(s.command_state(), CommandState::Unknown))
            .filter_map(|(pid, _)| self.backend_sessions.get(pid).map(|s| s.as_str()))
            .collect();
        let restored_busy =
            tako_control::agents::count_sessions_with_running_children(&unknown_backends);
        let busy_agents = running_count + restored_busy;
        let state = tako_control::sleep_guard::update(
            settings.sleep_guard_mode,
            settings.sleep_guard_power,
            settings.lid_sleep_mode,
            busy_agents,
        );
        self.sleep_guard_active = state.assertion_held;
        self.lid_closed = state.lid_closed;
        self.lid_sleep_disabled = state.lid_sleep_disabled;
        self.thermal_warning = state.thermal_state.is_warning();
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
                // #308: タブ D&D 開始時に titlebar_dragging を解除して
                // #312 のウインドウ移動と競合しないようにする
                this.titlebar_dragging = false;
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
            (
                pane_id,
                drop_zone(
                    fx,
                    fy,
                    matches!(kind, DragKind::File | DragKind::ExternalFile),
                ),
            )
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
        self.tab_drop_target = None;
        self.tab_reorder_indicator = None;
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
                focus: Some(true),
            },
            PaneOrigin::User,
        );
        if let Err(e) = result {
            eprintln!("warning: ペインを移動できない: {e}");
        }
        cx.notify();
    }

    /// タブバーへのペイン D&D のドロップ先フィードバック更新（Issue #209）。
    /// `dest`: Some(tab_id) = 既存タブへ合流、None = 新タブ化（+ ボタン / 余白）
    pub(crate) fn set_tab_drop_target(&mut self, dest: Option<TabId>, cx: &mut Context<Self>) {
        let new = Some(dest);
        if self.tab_drop_target != new {
            self.tab_drop_target = new;
            cx.notify();
        }
    }

    /// タブ D&D 並べ替えの挿入インジケータ更新（#308）。
    /// `before`: Some(tab_id) = そのタブの左にインジケータ表示、None = 末尾
    pub(crate) fn set_tab_reorder_indicator(
        &mut self,
        before: Option<TabId>,
        cx: &mut Context<Self>,
    ) {
        let new = Some(before);
        if self.tab_reorder_indicator != new {
            self.tab_reorder_indicator = new;
            cx.notify();
        }
    }

    /// タブ D&D 並べ替えのドロップ確定（#308）。
    /// `before`: Some(tab_id) = そのタブの位置へ移動、None = 末尾
    pub(crate) fn drop_tab_reorder(
        &mut self,
        dragged: TabId,
        before: Option<TabId>,
        cx: &mut Context<Self>,
    ) {
        self.drag_kind = None;
        self.tab_reorder_indicator = None;
        let target_index = match before {
            Some(before_id) => self
                .workspace
                .tabs()
                .iter()
                .position(|t| t.id() == before_id)
                .unwrap_or(usize::MAX),
            None => usize::MAX,
        };
        if let Err(e) = self.workspace.move_tab(dragged, target_index) {
            eprintln!("warning: タブを並べ替えできない: {e}");
        }
        self.save_layout();
        cx.notify();
    }

    /// タブバーへペインをドロップ（Issue #209）。
    /// `dest`: Some(tab_id) = 既存タブへ合流、None = 新タブ化
    pub(crate) fn drop_pane_on_tab(
        &mut self,
        pane: PaneId,
        dest: Option<TabId>,
        cx: &mut Context<Self>,
    ) {
        self.drag_kind = None;
        self.tab_drop_target = None;
        let result = match dest {
            Some(tab_id) => {
                if self.workspace.find_tab_of_pane(pane) == Some(tab_id) {
                    cx.notify();
                    return;
                }
                tako_control::dispatch(
                    self,
                    tako_control::protocol::Request::MovePane {
                        pane: Some(pane.as_u64()),
                        tab: Some(tab_id.as_u64()),
                        target: None,
                        direction: None,
                        focus: Some(true),
                    },
                    PaneOrigin::User,
                )
            }
            None => tako_control::dispatch(
                self,
                tako_control::protocol::Request::MovePane {
                    pane: Some(pane.as_u64()),
                    tab: None,
                    target: None,
                    direction: None,
                    focus: Some(true),
                },
                PaneOrigin::User,
            ),
        };
        if let Err(e) = result {
            eprintln!("warning: ペインをタブへ移動できない: {e}");
        }
        self.scroll_active_tab_into_view();
        cx.notify();
    }

    /// ファイルのドロップ（FR-3.11 / FR-3.13 / Issue #21）:
    /// ターミナルペイン中央 → パス文字列を send（複数はスペース区切り）、それ以外 → ファイルを開く。
    /// `cmd_held` が true の場合、ターミナルへのドロップで cd も実行する
    fn drop_files(
        &mut self,
        pane_id: PaneId,
        paths: &[std::path::PathBuf],
        cmd_held: bool,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let zone = self.take_drop_zone(pane_id).unwrap_or(DropZone::Center);
        let is_terminal =
            self.terminals.contains_key(&pane_id) && !self.previews.contains_key(&pane_id);
        if is_terminal && zone == DropZone::Center {
            let text = if cmd_held {
                let dir = if paths[0].is_dir() {
                    paths[0].clone()
                } else {
                    paths[0]
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| paths[0].clone())
                };
                format!(
                    "cd {}",
                    tako_core::quote_for_shell(&dir.display().to_string())
                )
            } else {
                tako_core::quote_paths_for_shell(paths)
            };
            let _ = tako_control::dispatch(
                self,
                tako_control::protocol::Request::Send {
                    pane: Some(pane_id.as_u64()),
                    text,
                    newline: cmd_held,
                    tmux_session: None,
                    await_prompt: false,
                },
                PaneOrigin::User,
            );
            cx.notify();
            return;
        }
        for (i, path) in paths.iter().enumerate() {
            let direction = if i == 0 {
                match zone {
                    DropZone::Center => None,
                    zone => Some(zone_to_direction(zone)),
                }
            } else {
                Some(tako_control::protocol::Direction::Right)
            };
            let result = tako_control::dispatch(
                self,
                tako_control::protocol::Request::OpenFile {
                    pane: Some(pane_id.as_u64()),
                    path: path.display().to_string(),
                    mode: None,
                    direction,
                    focus: Some(true),
                },
                PaneOrigin::User,
            );
            if let Err(e) = result {
                eprintln!("warning: ファイルを開けない: {e}");
            }
        }
        self.drain_pending_highlights(cx);
        cx.notify();
    }

    /// サイドバーやプレビュー領域への外部ファイルドロップ（Issue #21）:
    /// フォーカスペインの位置でファイルを開く（direction なし = 既存プレビュー再利用）
    fn open_dropped_files(&mut self, paths: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let focus_pane = self.workspace.active_tab().tree().focused();
        for path in paths {
            let result = tako_control::dispatch(
                self,
                tako_control::protocol::Request::OpenFile {
                    pane: Some(focus_pane.as_u64()),
                    path: path.display().to_string(),
                    mode: None,
                    direction: None,
                    focus: Some(true),
                },
                PaneOrigin::User,
            );
            if let Err(e) = result {
                eprintln!("warning: ファイルを開けない: {e}");
            }
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
                this.drop_files(pane_id, std::slice::from_ref(&drag.path), false, cx);
            }))
            .on_drag_move::<ExternalPaths>(cx.listener(
                move |this, e: &DragMoveEvent<ExternalPaths>, _, cx| {
                    this.update_drop_target(
                        pane_id,
                        e.bounds,
                        e.event.position,
                        DragKind::ExternalFile,
                        cx,
                    );
                },
            ))
            .on_drop::<ExternalPaths>(cx.listener(
                move |this, paths: &ExternalPaths, window, cx| {
                    let cmd_held = window.modifiers().platform;
                    this.drop_files(pane_id, paths.paths(), cmd_held, cx);
                },
            ))
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
            let is_terminal_pane =
                self.terminals.contains_key(&pane_id) && !self.previews.contains_key(&pane_id);
            let label = match (kind, zone) {
                (DragKind::TmuxSession, _) => "ここに分割して表示",
                (DragKind::ExternalFile, DropZone::Center) if is_terminal_pane => "パスを入力",
                (DragKind::File, DropZone::Center) if is_terminal_pane => "パスを入力",
                (DragKind::ExternalFile | DragKind::File, DropZone::Center) => "ここで開く",
                (DragKind::ExternalFile | DragKind::File, _) => "ここに分割して開く",
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

    fn on_pane_mouse_down(
        &mut self,
        pane_id: PaneId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.workspace.active_tab_mut().tree_mut().focus(pane_id);
        if let Some((col, row, _right)) = self.cell_at(pane_id, event.position, window) {
            let mirror_scrolling = self
                .scroll_ctls
                .get(&pane_id)
                .is_some_and(|c| c.mirror_scrolling());
            // cmd+クリック: リンクを開く（ミラースクロール表示中は視覚位置と
            // リンク検出座標が一致しないため判定しない。#159）
            if event.modifiers.platform && event.click_count == 1 && !mirror_scrolling {
                if let Some(link) = self.hovered_link.take() {
                    if link.contains(pane_id, row, col) {
                        self.open_link(&link.target, link.kind, pane_id, cx);
                        cx.notify();
                        return;
                    }
                }
                // cmd を押してからマウスを動かさずクリックした場合も、その場で最新画面を検出する
                self.refresh_pane_links(pane_id);
                let links = self.pane_links.get(&pane_id);
                if let Some(links) = links {
                    if let Some(link) = tako_core::link_at(links, row, col) {
                        let target = link.target.clone();
                        let kind = link.kind;
                        self.open_link(&target, kind, pane_id, cx);
                        cx.notify();
                        return;
                    }
                }
            }
            // バックエンドのミラースクロール表示中は選択を開始しない: 選択は
            // alacritty の viewport（ライブ画面）に対して働くため、ミラー行が
            // 見えている間は視覚位置と選択位置が一致しない（#159 の既知制約。
            // コピーは最下部へ戻ってから）
            if !mirror_scrolling {
                if let Some(session) = self.terminals.get(&pane_id) {
                    let kind = match event.click_count {
                        1 => SelectionKind::Simple,
                        2 => SelectionKind::Word,
                        _ => SelectionKind::Line,
                    };
                    session.clear_selection();
                    session.start_selection(kind, col, row, _right);
                    self.selecting = Some(pane_id);
                }
            }
        }
        cx.notify();
    }

    /// リンクを開く。URL はデフォルトブラウザ、パスはペイン分割して表示。
    /// 将来 webview ペインに差し替える場合はここを変更する。
    fn open_link(
        &mut self,
        target: &str,
        kind: tako_core::LinkKind,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) {
        match kind {
            tako_core::LinkKind::Url => {
                let _ = std::process::Command::new("open").arg(target).spawn();
            }
            tako_core::LinkKind::Path => {
                let path = std::path::Path::new(target);
                if path.is_dir() {
                    // ディレクトリ: 右に分割して cd
                    let result = tako_control::dispatch(
                        self,
                        tako_control::protocol::Request::Split {
                            pane: Some(pane_id.as_u64()),
                            tab: None,
                            direction: Some(tako_control::protocol::Direction::Right),
                            ratio: None,
                            command: None,
                            cwd: Some(target.to_string()),
                            focus: Some(true),
                        },
                        PaneOrigin::User,
                    );
                    match result {
                        Ok(_) => {
                            // UI から dispatch を直接呼ぶため、IPC / MCP ループと同じ
                            // pending_attach 後処理をここで実行する。これを欠くとツリー上に
                            // 空ペインだけができ、PTY も cwd も存在しない（#153）。
                            for (pane, options) in std::mem::take(&mut self.pending_attach) {
                                if let Err(e) = self.spawn_session(pane, options, cx) {
                                    eprintln!("warning: ディレクトリペインを開けない: {e}");
                                    self.remove_pane(pane, cx);
                                }
                            }
                            for (pane, data) in std::mem::take(&mut self.pending_writes) {
                                if let Some(session) = self.terminals.get(&pane) {
                                    session.write(data);
                                }
                            }
                        }
                        Err(e) => eprintln!("warning: ディレクトリを開けない: {e}"),
                    }
                } else {
                    // ファイル: 右に分割してプレビュー
                    let result = tako_control::dispatch(
                        self,
                        tako_control::protocol::Request::OpenFile {
                            pane: Some(pane_id.as_u64()),
                            path: target.to_string(),
                            mode: None,
                            direction: Some(tako_control::protocol::Direction::Right),
                            focus: Some(true),
                        },
                        PaneOrigin::User,
                    );
                    if let Err(e) = result {
                        eprintln!("warning: ファイルを開けない: {e}");
                    }
                }
                self.drain_pending_highlights(cx);
                cx.notify();
            }
        }
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
        let backend = self.mirror_scroll_pane(pane_id);
        let Some(session) = self.terminals.get(&pane_id) else {
            return;
        };
        let (_, rows) = session.size();
        let history = if backend {
            self.scroll_ctls
                .get(&pane_id)
                .map(|c| {
                    c.known_history
                        .max(c.mirror.as_ref().map(|m| m.total_history).unwrap_or(0))
                })
                .unwrap_or(0)
        } else {
            session.history_size()
        };
        let total = (history + rows) as f32;
        let ratio = ((f32::from(y) - f32::from(area.origin.y)) / f32::from(area.size.height))
            .clamp(0.0, 1.0);
        // 表示窓（rows 行）の中心をマウス位置の行へ合わせ、上端行 → offset に直す
        let top_row = (ratio * total - rows as f32 / 2.0).clamp(0.0, history as f32);
        let goal = history as f32 - top_row;
        if backend {
            // ミラー上の位置を直接動かす（未ロードなら目標だけ立ててロード開始。#159）
            let ctl = self.scroll_ctls.entry(pane_id).or_default();
            ctl.last_activity = std::time::Instant::now();
            let mut need_pump = false;
            match ctl.mirror.as_mut() {
                Some(m) => {
                    m.position = goal.clamp(0.0, m.total_history as f32);
                    if m.position <= 0.0 {
                        ctl.mirror = None;
                    } else if m.wants_more_history(rows) && !ctl.loading {
                        need_pump = true;
                    }
                }
                None => {
                    ctl.pending_rows = goal.max(0.0);
                    if ctl.pending_rows > 0.0 && !ctl.loading {
                        need_pump = true;
                    }
                }
            }
            if need_pump {
                self.pump_mirror(pane_id, cx);
            }
            self.ensure_scroll_ticker(cx);
        } else {
            // 直接ペインは行小数のままドラッグ追従（サブライン描画。#159）
            session.scroll_to_position(goal);
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
        // cmd+ホバーでリンク検出（ボタン押下状態に関係なく判定）
        self.update_hovered_link_at(event.position, event.modifiers.platform, window, cx);
        // PDF プレビューのリンクホバー（#271）
        self.update_pdf_link_hover(event.position, event.modifiers.platform, cx);

        if event.pressed_button != Some(MouseButton::Left) {
            // ウィンドウ外でボタンが離されると MouseUp が届かないことがある。
            // 取り残したドラッグ・選択状態はここで畳む（残留すると以後どこを
            // 左ドラッグしてもリサイズが発火し「当たり判定が広がった」ように見える）
            if self.dragging_border.take().is_some()
                | self.dragging_scrollbar.take().is_some()
                | self.selecting.take().is_some()
                | self.drag_scroll.take().is_some()
                | self.preview_selecting.take().is_some()
                | self.dragging_pin.take().is_some()
                | std::mem::take(&mut self.dragging_panel)
                | std::mem::take(&mut self.dragging_sidebar)
                | self.video_seek_dragging.take().is_some()
            {
                cx.notify();
            }
            return;
        }
        // シークバードラッグ中はシーク位置を追従
        if let Some(pane_id) = self.video_seek_dragging {
            self.video_seek_by_drag(pane_id, event.position, cx);
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
        // 左サイドバーの幅ドラッグ（Issue #307。右パネルと同方式）
        if self.dragging_sidebar {
            let total = f32::from(window.viewport_size().width);
            let max = (total * 0.5).max(SIDEBAR_MIN_WIDTH);
            self.sidebar_width = f32::from(event.position.x).clamp(SIDEBAR_MIN_WIDTH, max);
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
                }
                self.sync_editor_selection_from_preview(pid);
                cx.notify();
            }
            return;
        }
        let Some(pane_id) = self.selecting else {
            return;
        };
        // ドラッグ選択はテキスト領域を外れても最寄りセルへ伸ばし続ける（クランプ版）
        if let Some((col, row, right)) = self.cell_at_clamped(pane_id, event.position, window) {
            if let Some(session) = self.terminals.get(&pane_id) {
                session.extend_selection(col, row, right);
                cx.notify();
            }
        }
        // ドラッグ選択中のオートスクロール判定（#310）
        self.update_drag_scroll(pane_id, event.position, cx);
    }

    /// ドラッグ選択中のオートスクロール状態を更新する（#310）。
    /// マウスがペイン上下端の DRAG_SCROLL_MARGIN 範囲内ならスクロール開始、
    /// 範囲外に戻ったら停止する
    fn update_drag_scroll(
        &mut self,
        pane_id: PaneId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        // alt_screen（全画面 TUI）ではスクロールバックがないため自動スクロールしない
        if let Some(session) = self.terminals.get(&pane_id) {
            if session.is_alt_screen() {
                self.drag_scroll = None;
                return;
            }
        }
        let Some((_, area)) = self.pane_text_areas.iter().find(|(id, _)| *id == pane_id) else {
            return;
        };
        let area = *area;
        let y = f32::from(position.y);
        let top = f32::from(area.origin.y);
        let bottom = top + f32::from(area.size.height);

        let speed_factor = if y < top + DRAG_SCROLL_MARGIN {
            // 上端に近い → 過去方向（正）へスクロール
            let dist = (top + DRAG_SCROLL_MARGIN - y).min(DRAG_SCROLL_MARGIN);
            dist / DRAG_SCROLL_MARGIN
        } else if y > bottom - DRAG_SCROLL_MARGIN {
            // 下端に近い → 未来方向（負）へスクロール
            let dist = (y - (bottom - DRAG_SCROLL_MARGIN)).min(DRAG_SCROLL_MARGIN);
            -(dist / DRAG_SCROLL_MARGIN)
        } else {
            0.0
        };

        if speed_factor == 0.0 {
            self.drag_scroll = None;
            return;
        }

        let was_none = self.drag_scroll.is_none();
        self.drag_scroll = Some(DragScrollState {
            pane: pane_id,
            speed_factor,
        });
        if was_none {
            self.start_drag_scroll_timer(cx);
        }
    }

    /// ドラッグ選択オートスクロールのタイマーを起動する
    fn start_drag_scroll_timer(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(DRAG_SCROLL_INTERVAL_MS))
                .await;
            let should_continue = this
                .update(cx, |this, cx| this.tick_drag_scroll(cx))
                .unwrap_or(false);
            if !should_continue {
                break;
            }
        })
        .detach();
    }

    /// ドラッグ選択オートスクロールの 1 ティック分を処理する
    fn tick_drag_scroll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(state) = &self.drag_scroll else {
            return false;
        };
        if self.selecting != Some(state.pane) {
            self.drag_scroll = None;
            return false;
        }
        let pane_id = state.pane;
        let factor = state.speed_factor;
        let delta_rows = drag_scroll_delta(factor.abs());

        let session = match self.terminals.get(&pane_id) {
            Some(s) => s,
            None => {
                self.drag_scroll = None;
                return false;
            }
        };
        let (cols, rows) = session.size();

        if factor > 0.0 {
            // 過去方向（上）
            session.scroll_display(delta_rows.ceil() as i32);
            session.extend_selection(0, 0, false);
        } else {
            // 未来方向（下）
            session.scroll_display(-(delta_rows.ceil() as i32));
            session.extend_selection(cols.saturating_sub(1), rows.saturating_sub(1), true);
        }
        cx.notify();
        true
    }

    /// cmd+ホバー時のリンク検出更新
    fn update_hovered_link_at(
        &mut self,
        position: Point<Pixels>,
        cmd: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old = self.hovered_link.is_some();

        if !cmd {
            if self.hovered_link.take().is_some() {
                cx.notify();
            }
            return;
        }

        // マウス位置がどのペインのどのセルか判定
        let mut found = None;
        for &(pane_id, _) in &self.pane_text_areas {
            if let Some((col, row, _)) = self.cell_at(pane_id, position, window) {
                // ミラースクロール表示中はリンク判定しない: 検出はライブ viewport
                // ベースのため、ミラー行が見えている間は視覚位置と一致しない（#159）
                if self
                    .scroll_ctls
                    .get(&pane_id)
                    .is_some_and(|c| c.mirror_scrolling())
                {
                    break;
                }
                // リンクキャッシュを更新（ペインの画面が変わるたびにリフレッシュ）
                self.refresh_pane_links(pane_id);
                if let Some(links) = self.pane_links.get(&pane_id) {
                    if let Some(link) = tako_core::link_at(links, row, col) {
                        found = Some(HoveredLink {
                            pane: pane_id,
                            target: link.target.clone(),
                            kind: link.kind,
                            spans: link.spans.clone(),
                        });
                    }
                }
                break;
            }
        }

        let changed = match (&self.hovered_link, &found) {
            (Some(a), Some(b)) => a.target != b.target || a.pane != b.pane,
            (None, None) => false,
            _ => true,
        };
        self.hovered_link = found;
        if changed || old {
            cx.notify();
        }
    }

    /// cmd 単独の押下・解放でも、現在のマウス位置にあるリンク装飾を即時更新する。
    fn on_modifiers_changed(
        &mut self,
        event: &gpui::ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_hovered_link_at(
            window.mouse_position(),
            event.modifiers.platform,
            window,
            cx,
        );
        // PDF プレビューのリンクホバーも更新（#271）
        self.update_pdf_link_hover(window.mouse_position(), event.modifiers.platform, cx);
    }

    /// ペインのリンク検出キャッシュを更新する
    fn refresh_pane_links(&mut self, pane_id: PaneId) {
        // Issue #168: cmd+ホバー毎の画面スナップショット + 正規表現走査 + パス実在
        // チェック（syscall）のコストを計測
        let _span = tako_control::diag::perf_span("link_scan");
        let Some(session) = self.terminals.get(&pane_id) else {
            return;
        };
        let screen = session.screen(&self.theme);
        let cwd = session.cwd().map(std::path::Path::to_path_buf);
        let links = tako_core::detect_links_with_cwd(&screen, cwd.as_deref());
        self.pane_links.insert(pane_id, links);
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
        // D&D の後始末（ドロップ成立時は on_drop が stop_propagation 込みで先に畳む。
        // ここはドロップ先以外で離した場合のクリア）
        if self.drag_kind.take().is_some()
            | self.drop_target.take().is_some()
            | self.tab_drop_target.take().is_some()
            | self.tab_reorder_indicator.take().is_some()
        {
            cx.notify();
            return;
        }
        let sidebar_was_dragging = std::mem::take(&mut self.dragging_sidebar);
        if self.dragging_border.take().is_some()
            | self.dragging_scrollbar.take().is_some()
            | self.dragging_pin.take().is_some()
            | std::mem::take(&mut self.dragging_panel)
            | sidebar_was_dragging
            | self.video_seek_dragging.take().is_some()
        {
            if sidebar_was_dragging {
                self.save_sidebar_width();
            }
            cx.notify();
            return;
        }
        self.preview_selecting = None;
        self.drag_scroll = None;
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

    /// ペインのスクロール実体が tmux 側にあるか（tako 管理のバックエンドセッション、
    /// または `tako tmux open` で表示している外部セッション）。ミラースクロール・
    /// スクロールバー・CLI/MCP Scroll の分岐はすべてこれで判定する（#181: 再アタッチ・
    /// ビューラッパーペインは外側 alacritty が alt screen（履歴なし）のため、
    /// backend_sessions だけの判定では直接ペイン扱いに落ちてスクロール不能だった）
    fn mirror_scroll_pane(&self, pane_id: PaneId) -> bool {
        self.backend_sessions.contains_key(&pane_id) || self.tmux_view_panes.contains_key(&pane_id)
    }

    /// ミラースクロールの実体解決の起点。**TmuxOpen ビューを最優先**する: persist ON では
    /// ビューペインの外側 PTY（tmux attach クライアント）も spawn 時に backend セッションで
    /// ラップされ backend_sessions に入るが、それは輸送層で、表示実体（履歴）は常にビュー先に
    /// ある。backend を先に見ると resolve_target が外側ラッパー（history 0、かつネスト候補は
    /// 既定サーバーのみで別 socket のビュー先を辿れない）へ解決しスクロール不能になる
    /// （#181 実機「効かない」の第二の根因。persist OFF の隔離検証だけでは踏まない）。
    /// ビュー先は wrapper（無ければ元セッション）@ socket がそのまま実体
    /// （grouped session の表示 window は wrapper 側にあるため）。
    /// バックエンドペインは従来どおりネスト tmux を辿る resolve が必要
    fn mirror_source(&self, pane_id: PaneId) -> Option<MirrorSource> {
        if let Some(view) = self.tmux_view_panes.get(&pane_id) {
            return Some(MirrorSource::Fixed(
                tako_core::scroll::ScrollTarget::Nested {
                    socket: view.socket.clone(),
                    session: view.wrapper.clone().unwrap_or_else(|| view.session.clone()),
                },
            ));
        }
        let backend = self.backend_sessions.get(&pane_id)?;
        Some(MirrorSource::Backend(backend.clone()))
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
        // トラックパッドは Pixels（そのまま行小数へ）、マウスホイールは Lines
        // （1 ノッチ = 3 行。iTerm2 / Terminal.app と同等）。慣性は macOS が
        // momentum イベントとして Pixels デルタを流し続けるため、加算だけで効く
        let delta_rows = match event.delta {
            ScrollDelta::Lines(l) => l.y * 3.0,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(cell.height),
        };
        let (col, row) = self
            .cell_at(pane_id, event.position, window)
            .map(|(c, r, _)| (c, r))
            .unwrap_or((0, 0));
        if self.mirror_scroll_pane(pane_id) {
            // バックエンド / TmuxOpen ビューペイン: tmux 履歴のローカルミラー上で
            // ピクセル単位にスクロールする（マウス要求アプリへの SGR 転送も内部で出し分け。#159/#181）
            self.backend_scroll_px(pane_id, delta_rows, (col, row), cx);
        } else if let Some(session) = self.terminals.get(&pane_id) {
            // 直接ペイン: 行小数のままセッションへ（ピクセル単位スムーススクロール #159）。
            // mouse reporting / alternate scroll の転送とスクロールバック表示の
            // 出し分け・転送時の整数化はセッション側（scroll_wheel_px）
            session.scroll_wheel_px(delta_rows, col, row);
            self.mark_scroll_activity(pane_id, cx);
            cx.notify();
        }
    }

    // --- バックエンド / ネスト tmux スクロール（ローカルミラー方式。#159） ---

    /// ホイールの行小数をバックエンドスクロールへ積む。マウス要求アプリ（vim 等）へは
    /// 従来どおり生 SGR を転送し、それ以外は tmux 履歴のローカルミラー
    /// （`tako-core::scroll_mirror`）上でピクセル単位にスクロールする。
    /// 旧方式（tmux copy-mode 駆動）は ① 行単位でしか動けない ② 1 操作 = tmux 往復
    /// 数十 ms で慣性に追従できない ③ copy-mode 滞在のキー飲まれ、が原理的に残るため
    /// 置き換えた（外側 alacritty に履歴は積もらないことを 2026-07-13 実測で確認済み）
    fn backend_scroll_px(
        &mut self,
        pane_id: PaneId,
        delta_rows: f32,
        cell: (usize, usize),
        cx: &mut Context<Self>,
    ) {
        let rows = self
            .terminals
            .get(&pane_id)
            .map(|s| s.size().1)
            .unwrap_or(24);
        let ctl = self.scroll_ctls.entry(pane_id).or_default();
        ctl.last_activity = std::time::Instant::now();
        ctl.last_cell = cell;
        let wants_mouse = ctl.wants_mouse;
        let mut need_pump = false;
        if wants_mouse == Some(true) {
            // マウス要求アプリへのレポート: 整数行に畳み（行未満は持ち越し）、
            // レート制限を通して tmux サーバーへ直接注入する（send-keys -H。#167）
            let carry = self.scroll_accum.entry(pane_id).or_insert(0.0);
            let (lines, rest) = accumulate_scroll(*carry, delta_rows);
            *carry = rest;
            let allowed = match self.terminals.get(&pane_id) {
                Some(session) if lines != 0 => session.take_wheel_budget(lines),
                _ => 0,
            };
            if allowed != 0 {
                if let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) {
                    ctl.pending_wheel += allowed;
                }
                self.pump_wheel(pane_id, cx);
            }
        } else if let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) {
            if let Some(m) = ctl.mirror.as_mut() {
                m.scroll_by(delta_rows);
                if m.position <= 0.0 {
                    // 最下部到達でライブ表示へ復帰
                    ctl.mirror = None;
                } else if m.wants_more_history(rows) && !ctl.loading {
                    need_pump = true;
                }
            } else if delta_rows > 0.0 {
                // 過去方向の初動: ロード完了まで蓄積
                ctl.pending_rows += delta_rows;
                if !ctl.loading {
                    need_pump = true;
                }
            }
        }
        if need_pump {
            self.pump_mirror(pane_id, cx);
        }
        self.ensure_scroll_ticker(cx);
        cx.notify();
    }

    /// tmux 直接注入（send-keys -H）待ちのホイールイベントを非同期送信する（#167）。
    /// in-flight 中は `pending_wheel` に溜め、完了時に残りを再送する（ペイン単位に直列 =
    /// イベント順序の保証 + サブプロセス起動レートの自動調整）
    fn pump_wheel(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) else {
            return;
        };
        if ctl.wheel_sending || ctl.pending_wheel == 0 {
            return;
        }
        let Some(target) = ctl.target.clone() else {
            // wants_mouse 解決前（target 未解決）は送らない（finish_mirror_load 経由で来る）
            ctl.pending_wheel = 0;
            return;
        };
        let lines = std::mem::take(&mut ctl.pending_wheel);
        let sgr = ctl.wants_sgr;
        let cell = ctl.last_cell;
        ctl.wheel_sending = true;
        cx.spawn(async move |this, cx| {
            let task = cx.background_executor().spawn(async move {
                tako_core::scroll_mirror::send_wheel(&target, lines, cell.0, cell.1, sgr);
            });
            task.await;
            this.update(cx, |app, cx| {
                if let Some(ctl) = app.scroll_ctls.get_mut(&pane_id) {
                    ctl.wheel_sending = false;
                }
                app.pump_wheel(pane_id, cx);
            })
            .ok();
        })
        .detach();
    }

    /// ミラーの解決・チャンク取得を非同期実行する（ペイン単位に直列）。
    /// 初回はスクロール実体の解決 + マウス要求判定 + 最新チャンク取得、
    /// 以降はさらに過去のチャンクを先頭へ足す。完了時に必要なら再ポンプする
    fn pump_mirror(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(source) = self.mirror_source(pane_id) else {
            return;
        };
        let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) else {
            return;
        };
        if ctl.loading || ctl.wants_mouse == Some(true) {
            return;
        }
        ctl.loading = true;
        ctl.last_refresh = std::time::Instant::now();
        let target = ctl.target.clone();
        let first_load = ctl.target.is_none();
        let skip = ctl.mirror.as_ref().map(|m| m.lines.len()).unwrap_or(0);
        let theme = self.theme.clone();
        let socket = tako_core::tmux_backend::socket_name();
        cx.spawn(async move |this, cx| {
            let task = cx.background_executor().spawn(async move {
                use tako_core::{scroll, scroll_mirror};
                let target = target.unwrap_or_else(|| match source {
                    MirrorSource::Backend(backend) => {
                        // ネスト候補は既定サーバー + backend socket 自身（#181: persist 復元で
                        // 戻ったビューペインは tmux_view_panes に載らないが、外側 PTY の
                        // tmux client（`--socket tako` のビュー先 = backend socket 上）を
                        // tty 突き合わせで検出してビュー先セッションへ解決できる）
                        scroll::resolve_target(&socket, &backend, &[None, Some(&socket)])
                    }
                    MirrorSource::Fixed(t) => t,
                });
                // 旧 tako（copy-mode 方式）や CLI が copy-mode を残していたら初回に掃除する
                // （新方式は copy-mode に入らないため、居残りはキー飲まれ事故になる）
                if first_load {
                    scroll::cancel(&target);
                }
                let state = scroll_mirror::history_state(&target);
                let chunk = match state {
                    Some(s) if !s.mouse && s.history > 0 => scroll_mirror::capture_history(
                        &target,
                        skip,
                        scroll_mirror::MIRROR_CHUNK,
                        &theme,
                    ),
                    _ => None,
                };
                (target, state, chunk)
            });
            let (target, state, chunk) = task.await;
            this.update(cx, |app, cx| {
                app.finish_mirror_load(pane_id, target, state, chunk, skip, cx);
            })
            .ok();
        })
        .detach();
    }

    /// `pump_mirror` の完了処理: ミラーへの統合・マウス要求判明時の SGR 振り替え・
    /// 必要に応じた再ポンプ
    fn finish_mirror_load(
        &mut self,
        pane_id: PaneId,
        target: tako_core::scroll::ScrollTarget,
        state: Option<tako_core::scroll_mirror::HistoryState>,
        chunk: Option<(Vec<tako_core::screen::ScreenLine>, usize)>,
        skip: usize,
        cx: &mut Context<Self>,
    ) {
        let rows = self
            .terminals
            .get(&pane_id)
            .map(|s| s.size().1)
            .unwrap_or(24);
        let mut flush_rows: Option<f32> = None;
        if let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) {
            ctl.loading = false;
            ctl.target = Some(target);
            match state {
                Some(tako_core::scroll_mirror::HistoryState {
                    history,
                    mouse,
                    sgr,
                }) => {
                    ctl.wants_mouse = Some(mouse);
                    ctl.wants_sgr = sgr;
                    ctl.known_history = history;
                    if mouse {
                        // 解決して初めてマウス要求アプリと判明: 溜まった分をレポートへ
                        flush_rows = Some(std::mem::take(&mut ctl.pending_rows));
                        ctl.mirror = None;
                    } else if let Some((lines, total)) = chunk {
                        let pending = std::mem::take(&mut ctl.pending_rows);
                        ctl.known_history = total;
                        match ctl.mirror.as_mut() {
                            Some(m) if skip > 0 => {
                                // 過去側チャンクを先頭へ（position は下端基準なので不変）
                                let mut joined = lines;
                                joined.append(&mut m.lines);
                                m.lines = joined;
                                m.total_history = total;
                                m.scroll_by(pending);
                            }
                            Some(m) => {
                                m.total_history = total;
                                m.scroll_by(pending);
                            }
                            None => {
                                let mut m = tako_core::scroll_mirror::ScrollMirror {
                                    lines,
                                    total_history: total,
                                    position: 0.0,
                                };
                                m.scroll_by(pending);
                                if m.position > 0.0 {
                                    ctl.mirror = Some(m);
                                }
                            }
                        }
                        if ctl.mirror.as_ref().is_some_and(|m| m.position <= 0.0) {
                            ctl.mirror = None;
                        }
                    } else {
                        // 履歴ゼロ（alt screen の TUI 等）: 蓄積は捨てる
                        ctl.pending_rows = 0.0;
                    }
                }
                None => {
                    // セッション消滅・tmux 不在
                    ctl.pending_rows = 0.0;
                    ctl.mirror = None;
                }
            }
        }
        if let Some(rows_f) = flush_rows {
            let carry = self.scroll_accum.entry(pane_id).or_insert(0.0);
            let (lines, rest) = accumulate_scroll(*carry, rows_f);
            *carry = rest;
            let allowed = match self.terminals.get(&pane_id) {
                Some(session) if lines != 0 => session.take_wheel_budget(lines),
                _ => 0,
            };
            if allowed != 0 {
                if let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) {
                    ctl.pending_wheel += allowed;
                }
                self.pump_wheel(pane_id, cx);
            }
        }
        let more = self.scroll_ctls.get(&pane_id).is_some_and(|c| {
            !c.loading
                && (c
                    .mirror
                    .as_ref()
                    .is_some_and(|m| m.wants_more_history(rows))
                    || (c.mirror.is_none() && c.pending_rows > 0.0 && c.wants_mouse != Some(true)))
        });
        if more {
            self.pump_mirror(pane_id, cx);
        }
        cx.notify();
    }

    /// ミラースクロール表示中に新規出力で tmux 履歴が伸びたら、押し出された行を
    /// 回収して表示内容を固定する（position を同じだけ進める = 内容アンカー）
    fn refresh_mirror(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) else {
            return;
        };
        if ctl.loading || !ctl.mirror_scrolling() {
            return;
        }
        let Some(target) = ctl.target.clone() else {
            return;
        };
        let known = ctl.mirror.as_ref().map(|m| m.total_history).unwrap_or(0);
        ctl.loading = true;
        ctl.last_refresh = std::time::Instant::now();
        let theme = self.theme.clone();
        cx.spawn(async move |this, cx| {
            let task = cx.background_executor().spawn(async move {
                use tako_core::scroll_mirror;
                let state = scroll_mirror::history_state(&target);
                let chunk = match state {
                    Some(s) if s.history > known => {
                        scroll_mirror::capture_history(&target, 0, s.history - known, &theme)
                    }
                    _ => None,
                };
                (state, chunk)
            });
            let (state, chunk) = task.await;
            this.update(cx, |app, cx| {
                if let Some(ctl) = app.scroll_ctls.get_mut(&pane_id) {
                    ctl.loading = false;
                    if let Some(s) = state {
                        ctl.known_history = s.history;
                        ctl.wants_mouse = Some(s.mouse);
                        ctl.wants_sgr = s.sgr;
                    }
                    if let (Some(m), Some((lines, total))) = (ctl.mirror.as_mut(), chunk) {
                        let added = lines.len() as f32;
                        m.lines.extend(lines);
                        m.total_history = total;
                        m.position = (m.position + added).min(total as f32);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// dispatch（CLI / MCP）の Scroll 実行後に必要なら非同期ロードを起動する
    /// （`ControlHost::backend_scroll_view` は同期処理のため spawn できない）
    fn sync_scroll_from_dispatch(&mut self, value: &serde_json::Value, cx: &mut Context<Self>) {
        let Some(pane) = value["pane"].as_u64() else {
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
        if self.mirror_scroll_pane(pane_id) {
            let need_load = self
                .scroll_ctls
                .get(&pane_id)
                .is_some_and(|c| c.mirror.is_none() && c.pending_rows > 0.0 && !c.loading);
            if need_load {
                self.pump_mirror(pane_id, cx);
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

    /// キー入力前にスクロールバック表示を最下部へ戻す（iTerm2 流）。
    /// ミラー方式では copy-mode に入らないためローカル状態を畳むだけで済む
    fn cancel_scroll_before_input(&mut self, pane_id: PaneId) {
        if let Some(ctl) = self.scroll_ctls.get_mut(&pane_id) {
            ctl.mirror = None;
            ctl.pending_rows = 0.0;
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

    /// シークバー上のドラッグ移動でシーク位置を追従する
    fn video_seek_by_drag(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let bar_bounds = self.video_seek_bar_bounds.get(&pane_id).copied();
        if let (Some(bounds), Some(player)) = (bar_bounds, self.video_players.get_mut(&pane_id)) {
            let frac = ((f32::from(position.x) - f32::from(bounds.origin.x))
                / f32::from(bounds.size.width))
            .clamp(0.0, 1.0);
            let new_time = frac as f64 * player.duration;
            player.seek(new_time);
            player.grab_frame();
            cx.notify();
        }
    }

    /// フォーカスペインが動画プレビューならキーボードショートカットを処理する。
    /// 処理した場合 true を返す
    fn handle_video_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let pane_id = self.focused_pane();
        if !self.video_players.contains_key(&pane_id) {
            return false;
        }
        let key = keystroke.key.as_str();
        let shift = keystroke.modifiers.shift;
        match key {
            "space" => {
                if let Some(p) = self.video_players.get_mut(&pane_id) {
                    p.toggle();
                    self.ensure_video_ticker(cx);
                    cx.notify();
                }
                true
            }
            "left" => {
                let delta = if shift { -10.0 } else { -5.0 };
                if let Some(p) = self.video_players.get_mut(&pane_id) {
                    p.seek_relative(delta);
                    p.grab_frame();
                    cx.notify();
                }
                true
            }
            "right" => {
                let delta = if shift { 10.0 } else { 5.0 };
                if let Some(p) = self.video_players.get_mut(&pane_id) {
                    p.seek_relative(delta);
                    p.grab_frame();
                    cx.notify();
                }
                true
            }
            "," => {
                if let Some(p) = self.video_players.get_mut(&pane_id) {
                    p.pause();
                    p.seek_relative(-1.0 / 30.0);
                    p.grab_frame();
                    cx.notify();
                }
                true
            }
            "." => {
                if let Some(p) = self.video_players.get_mut(&pane_id) {
                    p.pause();
                    p.seek_relative(1.0 / 30.0);
                    p.grab_frame();
                    cx.notify();
                }
                true
            }
            "m" => {
                if let Some(p) = self.video_players.get_mut(&pane_id) {
                    p.toggle_mute();
                    cx.notify();
                }
                true
            }
            "l" => {
                if let Some(p) = self.video_players.get_mut(&pane_id) {
                    p.toggle_loop();
                    cx.notify();
                }
                true
            }
            _ => false,
        }
    }

    /// スクロールバーのフェード再描画とミラーの増分追従。
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
                        let hovered = app.hovered_scrollbar;
                        app.scroll_ctls.retain(|id, ctl| {
                            ctl.loading
                                || ctl.pending_rows != 0.0
                                || ctl.mirror.is_some()
                                || Some(*id) == dragging
                                || Some(*id) == hovered
                                || scrollbar_alpha(ctl.last_activity.elapsed().as_millis()) > 0.0
                        });
                        // ミラー表示中は新規出力での履歴増（押し出し行）に追従して
                        // 表示内容を固定する
                        let refresh: Vec<PaneId> = app
                            .scroll_ctls
                            .iter()
                            .filter(|(_, ctl)| {
                                ctl.mirror_scrolling()
                                    && !ctl.loading
                                    && ctl.last_refresh.elapsed() >= Duration::from_millis(1000)
                            })
                            .map(|(id, _)| *id)
                            .collect();
                        for id in refresh {
                            app.refresh_mirror(id, cx);
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

    /// スクロールバーの描画情報 (top, thumb_h, track_h, alpha, 強調)。
    /// スクロール活動が無い・フェードアウト済み・履歴ゼロでは None（iTerm2 流）。
    /// ドラッグ / ホバー中は表示を維持しサムを強調する（macOS 慣行）
    fn scrollbar_overlay(
        &self,
        pane_id: PaneId,
        area: Bounds<Pixels>,
    ) -> Option<(f32, f32, f32, f32, bool)> {
        let emphasized =
            self.dragging_scrollbar == Some(pane_id) || self.hovered_scrollbar == Some(pane_id);
        let ctl = self.scroll_ctls.get(&pane_id)?;
        let alpha = if emphasized {
            1.0
        } else {
            scrollbar_alpha(ctl.last_activity.elapsed().as_millis())
        };
        if alpha <= 0.0 {
            return None;
        }
        let session = self.terminals.get(&pane_id)?;
        let (offset, history) = if self.mirror_scroll_pane(pane_id) {
            // バックエンドのスクロールバックは tmux 側（ネスト先含む）のミラー
            (
                ctl.mirror
                    .as_ref()
                    .map(|m| m.effective_position())
                    .unwrap_or(0.0),
                ctl.known_history
                    .max(ctl.mirror.as_ref().map(|m| m.total_history).unwrap_or(0)),
            )
        } else {
            if session.is_alt_screen() {
                return None;
            }
            // サブライン位置（行小数）でサムを連続移動させる（#159）
            (session.scroll_position(), session.history_size())
        };
        if history == 0 {
            return None;
        }
        let (_, rows) = session.size();
        let total = (history + rows) as f32;
        let track_h = f32::from(area.size.height);
        // サム最小長（20px）より低い極小領域では描かない（clamp の min > max panic 防止。#181）
        if track_h < 20.0 {
            return None;
        }
        let thumb_h = (rows as f32 / total * track_h).clamp(20.0, track_h);
        let top = ((history as f32 - offset.min(history as f32)) / total * track_h)
            .min(track_h - thumb_h);
        Some((top, thumb_h, track_h, alpha, emphasized))
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

    /// ch のテーマフォントのグリフ advance が半角セル幅と一致するか（Issue #64）。
    /// テーマフォントにグリフが無い文字（⏺ ⎿ 等）はフォールバックフォントで描画され
    /// advance がセル幅とずれる。そのままグループ化するとずれが累積して後続文字を
    /// 押し出すため、この判定でグループから除外して個別 div（セル幅固定）に隔離する
    fn glyph_snaps_to_cell(&self, ch: char) -> bool {
        // ASCII 印字文字はモノスペースフォント自身のグリフで advance == セル幅
        if ch.is_ascii() {
            return true;
        }
        if let Some(&snaps) = self.glyph_snap_cache.borrow().get(&ch) {
            return snaps;
        }
        // テーマフォントでの advance を実測し、セル幅基準の 'M' と比較する。
        // フォントにグリフが無ければ advance() が Err を返す（フォールバック解決は
        // シェイプ時にしか起きない）ので、不一致扱いに倒れる
        let snaps = (|| {
            let font = Font {
                family: SharedString::from(self.theme.font_family.clone()),
                ..gpui::font(self.theme.font_family.clone())
            };
            let font_id = self.text_system.resolve_font(&font);
            let fs = px(self.theme.font_size);
            let cw = self.text_system.advance(font_id, fs, 'M').ok()?.width;
            let adv = self.text_system.advance(font_id, fs, ch).ok()?.width;
            Some((adv - cw).abs() <= cw * 0.02)
        })()
        .unwrap_or(false);
        self.glyph_snap_cache.borrow_mut().insert(ch, snaps);
        snaps
    }

    /// 1 行分の文字ごとの描画情報（スタイルラン・セル幅・セル幅整合）を組み立てる。
    /// `terminal_screen_lines` の描画とセルフテストの検証で共用する
    fn line_char_infos(&self, line: &tako_core::screen::ScreenLine) -> Vec<CharInfo> {
        let chars: Vec<(usize, char)> = line.text.char_indices().collect();
        let mut infos: Vec<CharInfo> = Vec::with_capacity(chars.len());
        let mut run_idx = 0;
        for (ci, &(byte_off, ch)) in chars.iter().enumerate() {
            while run_idx + 1 < line.runs.len() && byte_off >= line.runs[run_idx].range.end {
                run_idx += 1;
            }
            let (cur_run_idx, bg) = if run_idx < line.runs.len()
                && byte_off >= line.runs[run_idx].range.start
                && byte_off < line.runs[run_idx].range.end
            {
                (run_idx, line.runs[run_idx].bg)
            } else {
                (usize::MAX, None)
            };
            let char_cols = if ci + 1 < line.cell_cols.len() {
                line.cell_cols[ci + 1] - line.cell_cols[ci]
            } else {
                1
            };
            infos.push(CharInfo {
                ch,
                char_cols,
                run_idx: cur_run_idx,
                bg,
                snaps: char_cols == 1 && self.glyph_snaps_to_cell(ch),
            });
        }
        infos
    }

    /// バックエンドペインのミラースクロール表示行列（#159）。
    /// tmux 履歴ミラー（古い→新しい）とライブ画面を連結した仮想空間から、
    /// 表示位置 pos（下端からの遡り行数）の窓 rows + 1 行（部分行込み）を切り出す。
    /// ミラー未使用・最下部表示では None（通常の viewport 描画へ）
    fn compose_mirror_lines(
        &self,
        pane_id: PaneId,
        screen: &tako_core::Screen,
    ) -> Option<Vec<tako_core::ScreenLine>> {
        if !self.mirror_scroll_pane(pane_id) {
            return None;
        }
        let ctl = self.scroll_ctls.get(&pane_id)?;
        let m = ctl.mirror.as_ref()?;
        let pos = m.effective_position();
        if pos <= 0.0 {
            return None;
        }
        let rows = screen.rows;
        let m_len = m.lines.len();
        // 表示窓の上端 = 仮想 index (m_len - ceil(pos))。ceil(pos) <= m_len は
        // effective_position のクランプで保証される
        let base = (pos.ceil() as usize).min(m_len);
        let start = m_len - base;
        Some(
            (start..start + rows + 1)
                .filter_map(|i| {
                    if i < m_len {
                        Some(m.lines[i].clone())
                    } else {
                        screen.lines.get(i - m_len).cloned()
                    }
                })
                .collect(),
        )
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
        // cmd+ホバー中のリンクスパン（行番号→(start_col, end_col) のマップ）
        let link_spans: HashMap<usize, (usize, usize)> = self
            .hovered_link
            .as_ref()
            .filter(|h| h.pane == pane_id)
            .map(|h| {
                h.spans
                    .iter()
                    .map(|&(row, sc, ec)| (row, (sc, ec)))
                    .collect()
            })
            .unwrap_or_default();

        let _total_cols = screen.cols;
        // 表示行列の決定（#159）:
        // - バックエンドのミラースクロール中: tmux 履歴ミラー + ライブ画面の合成
        // - それ以外: viewport（+ サブライン中は最下行の 1 行下 = extra_bottom）。
        //   描画側が行スタック全体を fract 行ぶん上へずらすため、下端の隙間を追加行が埋める
        let display_lines = self
            .compose_mirror_lines(pane_id, &screen)
            .unwrap_or_else(|| {
                let mut lines = screen.lines;
                lines.extend(screen.extra_bottom);
                lines
            });
        display_lines
            .into_iter()
            .enumerate()
            .map(|(row_idx, line)| {
                if cell_width.is_none() {
                    // セル幅未計測: フォールバック（起動直後の一瞬のみ）
                    let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = line
                        .runs
                        .iter()
                        .map(|run| (run.range.clone(), self.run_highlight(run)))
                        .collect();
                    return div().h(px(line_h)).whitespace_nowrap().child(
                        StyledText::new(line.text)
                            .with_default_highlights(&default_style, highlights),
                    );
                }
                // 同スタイルの連続半角文字をグループ化して描画要素数を削減。
                // 全角文字（char_cols > 1）とセル幅不一致グリフ（snaps == false）は
                // グリッドスナップのため個別 div に分離する。
                // 旧実装（1 文字 = 1 div）は描画プリミティブ数が cols×rows に
                // 比例し、GPUI bounds_tree 挿入の O(N log N) が支配的になって
                // セルフテスト等で実質ハングを引き起こしていた (#39)。
                // whitespace_nowrap は必須: グループのシェイプ幅が div 幅
                // （セル幅 × セル数）をヘアラインでも超えると GPUI のテキスト
                // レイアウトが折り返しを起こし、折り返された文字が行 div の
                // overflow_hidden の外に出て見えなくなる（Issue #64）
                let cw = cell_width.unwrap();
                let row = div()
                    .h(px(line_h))
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .whitespace_nowrap();
                let fallback_run = tako_core::screen::StyleRun {
                    range: 0..0,
                    fg: theme.foreground,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                    strikeout: false,
                    dim: false,
                };
                let run_highlights: Vec<HighlightStyle> =
                    line.runs.iter().map(|r| self.run_highlight(r)).collect();
                let infos = self.line_char_infos(&line);
                let chunks = chunk_line_chars(&infos);
                let mut children: Vec<gpui::AnyElement> = Vec::with_capacity(chunks.len());
                let link_span = link_spans.get(&row_idx);
                for chunk in chunks {
                    let hl = if chunk.run_idx < run_highlights.len() {
                        run_highlights[chunk.run_idx]
                    } else {
                        self.run_highlight(&fallback_run)
                    };
                    let text: String = infos[chunk.start..chunk.end].iter().map(|x| x.ch).collect();
                    let text_len = text.len();
                    let link_range = link_span.and_then(|&(link_sc, link_ec)| {
                        link_byte_range_in_chunk(&infos, &line.cell_cols, &chunk, link_sc, link_ec)
                    });
                    let highlights = if let Some(link_range) = link_range {
                        let mut link_hl = hl;
                        link_hl.underline = Some(UnderlineStyle {
                            thickness: px(1.0),
                            color: Some(hsla(theme.accent)),
                            wavy: false,
                        });
                        link_hl.color = Some(hsla(theme.accent));
                        link_hl.background_color = Some(hsla_alpha(theme.accent, 0.22));
                        let mut ranges = Vec::with_capacity(3);
                        if link_range.start > 0 {
                            ranges.push((0..link_range.start, hl));
                        }
                        ranges.push((link_range.clone(), link_hl));
                        if link_range.end < text_len {
                            ranges.push((link_range.end..text_len, hl));
                        }
                        ranges
                    } else {
                        vec![(0..text_len, hl)]
                    };
                    let styled = StyledText::new(SharedString::from(text))
                        .with_default_highlights(&default_style, highlights);
                    let mut d = div()
                        .w(cw * chunk.cols.max(1) as f32)
                        .flex_none()
                        .overflow_hidden();
                    if let Some(bg) = chunk.bg {
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

    fn render_webview_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: Bounds<Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let Some(idx) = self.webviews.iter().position(|e| e.pane == Some(pane_id)) else {
            // 呼び出し側で存在確認済み。万一消えていたら空枠だけ描く
            return div().id(("pane", pane_id.as_u64()));
        };
        let id = self.webviews[idx].id;
        let url = self.webviews[idx].current_url();
        let title = self.webviews[idx].current_title();
        let loading = self.webviews[idx].is_loading();

        // 本文領域 = pane_text_areas と同じ絶対座標（タイトルバー・枠・パディングの内側）。
        // GPUI の Pixels は論理座標なので wry の Logical bounds へそのまま渡せる。
        // 実描画はネイティブ webview 自身が行い、GPUI 側は枠とタイトルバーだけを描く
        self.webview_marks.insert(id);
        let bounds = (
            f64::from(f32::from(area.origin.x)),
            f64::from(f32::from(area.origin.y)),
            f64::from(f32::from(area.size.width)),
            f64::from(f32::from(area.size.height)),
        );
        self.webviews[idx].sync_frame(Some(bounds));

        let display_title = if title.trim().is_empty() {
            url.clone()
        } else {
            title
        };
        // ← / → / ⟳ のナビゲーションボタン（webview 本体はネイティブが処理するため、
        // GPUI 側 UI はタイトルバーに限られる）
        let nav_button = |icon: &'static str,
                          to: &'static str,
                          cx: &mut Context<Self>|
         -> gpui::Stateful<gpui::Div> {
            div()
                .id((to, pane_id.as_u64()))
                .w(px(20.0))
                .h(px(18.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.9))
                .hover(|d| {
                    d.bg(rgba_alpha(theme.surface_2, 0.9))
                        .text_color(hsla(theme.foreground))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if let Some(e) = this.webviews.iter().find(|e| e.pane == Some(pane_id)) {
                        if let Err(err) = e.navigate(to) {
                            eprintln!("warning: webview navigate({to}) 失敗: {err}");
                        }
                    }
                    cx.notify();
                }))
                .child(icon)
        };
        let body = div().flex_1().bg(rgba(theme.background));

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
                        self.drag_ghost_builder(DragKind::Pane, truncate(&display_title, 24), cx),
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
                                this.webview_close_button(pane_id, cx);
                            }))
                            .child("×"),
                    )
                    .child(
                        // ー = dock へ退避（ページは生きたまま。たまり場の ー と同じ作法）
                        div()
                            .id(("pane-web-hide", pane_id.as_u64()))
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                            .hover(|d| {
                                d.bg(rgba_alpha(theme.surface_2, 0.9))
                                    .text_color(hsla(theme.foreground))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.webview_hide_button(pane_id, cx);
                            }))
                            .child("ー"),
                    )
                    .child(nav_button("←", "back", cx))
                    .child(nav_button("→", "forward", cx))
                    .child(nav_button(if loading { "…" } else { "⟳" }, "reload", cx))
                    .child(
                        div()
                            .text_color(if focused {
                                hsla(theme.foreground)
                            } else {
                                hsla(theme.tab_inactive_foreground)
                            })
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                svg()
                                    .path(crate::file_icons::ui_icon::GLOBE)
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .text_color(hsla(theme.accent)),
                            )
                            .child(SharedString::from(truncate(&display_title, 32))),
                    )
                    .child(
                        // URL 表示（タイトルの右に控えめに。編集は CLI / MCP / cmd+K 経由）
                        div()
                            .flex_grow(1.0)
                            .overflow_hidden()
                            .text_size(px(10.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.7))
                            .child(SharedString::from(truncate(&url, 48))),
                    ),
            )
            .child(body)
    }

    /// Web ビューペインの × = ページごと完全破棄（ブラウザのタブ × と同じ）。
    /// 先に webviews から外す（remove_pane 側の dock 退避を発火させない）
    fn webview_close_button(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if let Some(idx) = self.webviews.iter().position(|e| e.pane == Some(pane_id)) {
            self.webviews.remove(idx);
        }
        self.remove_pane(pane_id, cx);
    }

    /// Web ビューペインの ー = ペインから外して dock へ退避（ページは生きたまま）。
    /// remove_pane 側の webview 後始末が pane 紐付けを解いて dock 落ちさせる
    fn webview_hide_button(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        self.remove_pane(pane_id, cx);
    }

    /// ペインから外れた Web ビューを dock 退避に落とす（ページ = wry インスタンスは
    /// 生かす。#155「ブラウザタブの維持」）。ペイン close の全経路
    /// （×・ー・タブ close・dispatch close）から呼ばれる。完全破棄は
    /// `webview_close_button` / dispatch `web close` が先に webviews から外して行う
    fn dock_webview_of(&mut self, pane: PaneId) {
        if let Some(e) = self.webviews.iter_mut().find(|e| e.pane == Some(pane)) {
            e.pane = None;
            e.sync_frame(None);
        }
    }

    /// wry WebView を生成して webviews に登録する（表示先ペインは呼び出し側が設定）
    fn create_webview(&mut self, url: &str) -> Result<webview::WebViewId, String> {
        let handle = self
            .window_raw_handle
            .as_ref()
            .ok_or("ウィンドウ初期化前のため Web ビューを作れない（直後に再試行）")?;
        let id = webview::WebViewId(self.webview_next_id);
        let entry = webview::WebViewEntry::build(handle, id, url)?;
        self.webview_next_id += 1;
        self.webviews.push(entry);
        Ok(id)
    }

    /// 表示中の Web ビューのタイトル・URL を JS 評価で更新する（2 秒ポーリング）。
    /// 結果はコールバック（次の runloop）で shared に届き、次回 render で反映される
    fn poll_webview_state(&self) {
        for e in &self.webviews {
            if e.pane.is_some() {
                e.poll_state();
            }
        }
    }

    /// dock の「表示」ボタン / dispatch 以外からの呼び出し口。表示中ならそのペインへ
    /// フォーカス（タブ切替込み）、dock 退避中ならフォーカスペインを右分割して表示する
    fn webview_show_from_dock(&mut self, id: webview::WebViewId, cx: &mut Context<Self>) {
        let Some(entry) = self.webviews.iter().find(|e| e.id == id) else {
            return;
        };
        if let Some(p) = entry.pane {
            let tab = self
                .workspace
                .tabs()
                .iter()
                .find(|t| t.tree().contains(p))
                .map(|t| t.id());
            if let Some(tab) = tab {
                if let Some(t) = self.workspace.get_tab_mut(tab) {
                    let _ = t.tree_mut().focus(p);
                }
                let _ = self.workspace.activate_tab(tab);
                self.scroll_active_tab_into_view();
            }
        } else {
            let target = self.workspace.active_tab().tree().focused();
            let new_pane = Pane::new(PaneOrigin::User);
            let new_id = new_pane.id();
            if self
                .workspace
                .active_tab_mut()
                .tree_mut()
                .split_with_ratio(target, SplitDirection::Right, 0.5, new_pane)
                .is_ok()
            {
                if let Some(e) = self.webviews.iter_mut().find(|e| e.id == id) {
                    e.pane = Some(new_id);
                }
                let _ = self.workspace.active_tab_mut().tree_mut().focus(new_id);
            }
        }
        cx.notify();
    }

    /// Web dock URL 入力欄のキー処理（#207）。dock が開いているときだけ呼ばれる。
    /// 入力を消費したら true を返す
    fn handle_webview_dock_url_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        match ks.key.as_str() {
            "enter" => {
                let url = self.webview_dock_url_input.trim().to_string();
                if url.is_empty() {
                    return true;
                }
                self.open_webview_from_dock(&url, cx);
                true
            }
            "escape" => {
                self.webview_dock_url_input.clear();
                self.webview_dock_url_cursor = 0;
                self.webview_dock_url_focused = false;
                self.webview_dock_open = false;
                cx.notify();
                true
            }
            "backspace" => {
                if self.webview_dock_url_cursor > 0 {
                    let prev = self.webview_dock_url_input[..self.webview_dock_url_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.webview_dock_url_input
                        .drain(prev..self.webview_dock_url_cursor);
                    self.webview_dock_url_cursor = prev;
                }
                cx.notify();
                true
            }
            "delete" => {
                if self.webview_dock_url_cursor < self.webview_dock_url_input.len() {
                    let next = self.webview_dock_url_cursor
                        + self.webview_dock_url_input[self.webview_dock_url_cursor..]
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(0);
                    self.webview_dock_url_input
                        .drain(self.webview_dock_url_cursor..next);
                }
                cx.notify();
                true
            }
            "left" => {
                if self.webview_dock_url_cursor > 0 {
                    self.webview_dock_url_cursor = self.webview_dock_url_input
                        [..self.webview_dock_url_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
                cx.notify();
                true
            }
            "right" => {
                if self.webview_dock_url_cursor < self.webview_dock_url_input.len() {
                    self.webview_dock_url_cursor += self.webview_dock_url_input
                        [self.webview_dock_url_cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                }
                cx.notify();
                true
            }
            _ => {
                if let Some(ch) = ks.key_char.as_deref() {
                    if !ch.chars().any(|c| c.is_control()) {
                        self.webview_dock_url_input
                            .insert_str(self.webview_dock_url_cursor, ch);
                        self.webview_dock_url_cursor += ch.len();
                        cx.notify();
                        return true;
                    }
                }
                true
            }
        }
    }

    /// URL を指定して Web ビューペインを開く（#207。dock UI からの共通入口）。
    /// create_webview + フォーカスペイン右分割で表示し、dock を閉じる
    fn open_webview_from_dock(&mut self, url: &str, cx: &mut Context<Self>) {
        let normalized = webview::normalize_url(url);
        match self.create_webview(&normalized) {
            Ok(id) => {
                self.webview_show_from_dock(id, cx);
                self.webview_dock_url_input.clear();
                self.webview_dock_url_cursor = 0;
                self.webview_dock_url_focused = false;
                self.webview_dock_open = false;
            }
            Err(e) => {
                eprintln!("warning: Web ビューを開けない: {e}");
            }
        }
        cx.notify();
    }

    /// Web ビュー dock（#155）。ステータスバーの Web ボタンで開閉する下部パネル。
    /// 全ページ（表示中 + 退避中）を一覧し、ワンクリックで呼び出し / 破棄できる。
    /// flex 列（ドロワーと同じ層）に挟まるためペインエリアが縮み、webview とは重ならない
    fn render_webview_dock(&mut self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.webview_dock_open {
            return None;
        }
        let theme = self.theme.clone();
        let entries: Vec<(webview::WebViewId, String, String, Option<PaneId>)> = self
            .webviews
            .iter()
            .map(|e| (e.id, e.current_title(), e.current_url(), e.pane))
            .collect();
        let rows: Vec<_> = entries
            .into_iter()
            .map(|(id, title, url, pane)| {
                let display_title = if title.trim().is_empty() {
                    url.clone()
                } else {
                    title
                };
                div()
                    .id(("webdock-row", id.as_u64()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(26.0))
                    .px(px(8.0))
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.webview_show_from_dock(id, cx);
                    }))
                    .child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(if pane.is_some() {
                                hsla(theme.accent)
                            } else {
                                hsla_alpha(theme.tab_inactive_foreground, 0.5)
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla(theme.foreground))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                svg()
                                    .path(crate::file_icons::ui_icon::GLOBE)
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .text_color(hsla(theme.accent)),
                            )
                            .child(SharedString::from(truncate(&display_title, 36))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(px(10.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.7))
                            .child(SharedString::from(truncate(&url, 56))),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.8))
                            .child(if pane.is_some() {
                                "表示中"
                            } else {
                                "退避中"
                            }),
                    )
                    .child(
                        div()
                            .id(("webdock-close", id.as_u64()))
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
                                if let Some(shown) = this.web_destroy(id.as_u64()) {
                                    this.remove_pane(shown, cx);
                                }
                                if this.webviews.is_empty() {
                                    this.webview_dock_open = false;
                                }
                                cx.notify();
                            }))
                            .child("×"),
                    )
            })
            .collect();
        Some(
            div()
                .flex_none()
                .w_full()
                .max_h(px(180.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .p(px(6.0))
                .bg(rgba(theme.surface_0))
                .border_t_1()
                .border_color(hsla(theme.border_subtle))
                .overflow_hidden()
                .children(rows)
                .child(self.render_webview_dock_url_input(&theme, cx)),
        )
    }

    /// Web dock URL 入力行の描画（#207）
    fn render_webview_dock_url_input(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let cursor = self
            .webview_dock_url_cursor
            .min(self.webview_dock_url_input.len());
        let before = &self.webview_dock_url_input[..cursor];
        let after = &self.webview_dock_url_input[cursor..];
        let display = if self.webview_dock_url_input.is_empty() {
            SharedString::from("URL を入力して Enter（例: example.com）")
        } else {
            SharedString::from(format!("{before}|{after}"))
        };
        let placeholder = self.webview_dock_url_input.is_empty();
        let focused = self.webview_dock_url_focused;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .h(px(28.0))
            .px(px(8.0))
            .child(
                svg()
                    .path(crate::file_icons::ui_icon::GLOBE)
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(hsla(theme.accent)),
            )
            .child(
                div()
                    .id("webdock-url-input")
                    .flex_1()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded_sm()
                    .cursor(CursorStyle::IBeam)
                    .bg(rgba_alpha(theme.accent, if focused { 0.12 } else { 0.08 }))
                    .border_1()
                    .border_color(hsla_alpha(theme.accent, if focused { 0.6 } else { 0.3 }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.webview_dock_url_focused = true;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .text_size(px(11.0))
                    .text_color(if placeholder {
                        hsla_alpha(theme.tab_inactive_foreground, 0.5)
                    } else {
                        hsla(theme.foreground)
                    })
                    .child(display),
            )
            .child(
                div()
                    .id("webdock-url-open")
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(rgba_alpha(theme.accent, 0.15))
                    .hover(|d| d.bg(rgba_alpha(theme.accent, 0.3)))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let url = this.webview_dock_url_input.trim().to_string();
                        if !url.is_empty() {
                            this.open_webview_from_dock(&url, cx);
                        }
                    }))
                    .child("開く"),
            )
    }

    /// render 末尾の可視性同期。今フレームで描画されなかった Web ビュー
    /// （非アクティブタブ・dock 退避中）と、D&D 中の全 Web ビューを隠す
    /// （ネイティブビューは GPUI のドロップターゲット描画より上に来るため）。
    /// 描画済み集合（webview_marks）はここで消費する
    fn sync_webview_visibility(&mut self, hide_all: bool) {
        let marks = std::mem::take(&mut self.webview_marks);
        for e in &mut self.webviews {
            if hide_all || !marks.contains(&e.id) {
                e.sync_frame(None);
            }
        }
    }

    /// 起動復元分の Web ビューを開き直す（初回 render でハンドル採取後に呼ぶ）。
    /// 保存時のペイン ID は Phase 5.5 の同一 ID 復元で現ワークスペースにも存在する
    fn restore_webviews(&mut self) {
        let pending = std::mem::take(&mut self.pending_webview_restore);
        for (pane, url) in pending {
            match self.create_webview(&url) {
                Ok(id) => {
                    if let Some(raw) = pane {
                        let target = self
                            .workspace
                            .tabs()
                            .iter()
                            .flat_map(|t| t.tree().panes())
                            .map(|p| p.id())
                            .find(|p| p.as_u64() == raw);
                        if target.is_none() {
                            eprintln!(
                                "warning: Web ビュー復元: ペイン {raw} が見つからないため dock へ退避 ({url})"
                            );
                        }
                        if let Some(e) = self.webviews.iter_mut().find(|e| e.id == id) {
                            e.pane = target;
                        }
                    }
                }
                Err(err) => eprintln!("warning: Web ビュー復元失敗 ({url}): {err}"),
            }
        }
    }

    /// 失敗ペインの「再実行」（#217 カンプ）。シェルの履歴呼び出し（上矢印）+
    /// Enter を送り、直前コマンドを再実行する
    fn retry_last_command(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if let Some(session) = self.terminals.get(&pane_id) {
            session.write(b"\x1b[A\r".to_vec());
        }
        cx.notify();
    }

    /// 子ワーカードロップダウン（#217 カンプ: w282 / radius 9 / ヘッダ + 行 + フッター）。
    /// master ペインの「N workers ▾」から開く。行クリックでジャンプ、
    /// フッターは master ペインへのフォーカス（起動指示の導線）
    fn render_workers_menu(
        &self,
        master_pane: PaneId,
        workers: &[WorkerRow],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        div()
            .absolute()
            .top(px(38.0))
            .left(px(140.0))
            .w(px(282.0))
            .rounded(px(9.0))
            .bg(rgba(theme.surface_1))
            .border_1()
            .border_color(hsla(theme.border_heavy))
            .shadow(vec![BoxShadow {
                color: gpui::hsla(0., 0., 0., 0.5),
                offset: point(px(0.), px(12.)),
                blur_radius: px(28.),
                spread_radius: px(0.),
                inset: false,
            }])
            .overflow_hidden()
            .occlude()
            .child(
                div()
                    .px(px(11.0))
                    .pt(px(8.0))
                    .pb(px(7.0))
                    .border_b_1()
                    .border_color(hsla(theme.border_subtle))
                    .text_size(px(9.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(hsla(theme.text_muted))
                    .child(SharedString::from(format!(
                        "MASTER の子ワーカー \u{00B7} {}",
                        workers.len()
                    ))),
            )
            .child(
                div()
                    .p(px(4.0))
                    .flex()
                    .flex_col()
                    .children(workers.iter().map(|w| {
                        let failed = matches!(w.state, tako_core::CommandState::Failed(_));
                        let dot_color = match w.state {
                            tako_core::CommandState::Failed(_) => theme.red,
                            tako_core::CommandState::Running => theme.accent,
                            tako_core::CommandState::Idle => theme.green,
                            tako_core::CommandState::Unknown => theme.text_overlay,
                        };
                        let target = w.pane;
                        div()
                            .id(("workers-menu-row", w.pane.as_u64()))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|d| {
                                if failed {
                                    d.bg(rgba_alpha(theme.red, 0.08))
                                } else {
                                    d.bg(rgba(theme.surface_hover_strong))
                                }
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.workers_menu_open = None;
                                this.jump_to_pane(target, cx);
                            }))
                            .child(if failed {
                                svg()
                                    .path(crate::file_icons::ui_icon::FAIL_X)
                                    .w(px(9.0))
                                    .h(px(9.0))
                                    .text_color(hsla(theme.red))
                                    .into_any_element()
                            } else {
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(hsla(dot_color))
                                    .into_any_element()
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(theme.font_family.clone())
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(truncate(&w.name, 20))),
                            )
                            .child(
                                div()
                                    .flex_grow(1.0)
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(10.5))
                                    .text_color(if failed {
                                        hsla(theme.red)
                                    } else {
                                        hsla(theme.text_muted)
                                    })
                                    .child(SharedString::from(if failed {
                                        "failed".to_string()
                                    } else {
                                        w.subtitle.clone()
                                    })),
                            )
                    })),
            )
            .child(
                div()
                    .id(("workers-menu-spawn", master_pane.as_u64()))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_t_1()
                    .border_color(hsla(theme.border_subtle))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.text_muted))
                    .cursor_pointer()
                    .hover(|d| {
                        d.text_color(hsla(theme.foreground))
                            .bg(rgba(theme.surface_hover))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.workers_menu_open = None;
                        // 起動指示は master との対話で行う（master ペインへ導線）
                        this.jump_to_pane(master_pane, cx);
                    }))
                    .child(
                        svg()
                            .path(crate::file_icons::ui_icon::PLUS)
                            .w(px(11.0))
                            .h(px(11.0))
                            .text_color(hsla(theme.text_muted)),
                    )
                    .child("ワーカーを起動"),
            )
            .into_any_element()
    }

    fn render_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: Bounds<Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // ネイティブ Web ビューペイン（FR-3.8 / #155）
        if self.webviews.iter().any(|e| e.pane == Some(pane_id)) {
            return self
                .render_webview_pane(pane_id, rect, area, focused, cx)
                .into_any_element();
        }
        // プレビューペイン（FR-3.2 / FR-3.3）はターミナルではなくファイル内容を描く
        if self.previews.contains_key(&pane_id) {
            return self
                .render_preview_pane(pane_id, rect, area, focused, cx)
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
        // role ラベル（カンプ: バッジではなく素のテキスト 9.5px 600 tracking 0.06em）
        let is_master = pane_role.as_deref().is_some_and(|r| {
            r.contains("orchestrator-master") || r == "master" || r.starts_with("master:")
        });
        let is_worker = pane_role
            .as_deref()
            .is_some_and(|r| r.contains("orchestrator-worker") || r.starts_with("worker"));
        let role_label = pane_role.as_deref().map(|r| {
            if is_master {
                ("ORCH".to_string(), theme.accent)
            } else if is_worker {
                ("WORKER".to_string(), theme.teal)
            } else {
                (
                    r.split(':').next().unwrap_or(r).to_uppercase(),
                    theme.text_tertiary,
                )
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
        let is_failed = matches!(state_label, Some("failed"));
        // 稼働時間（カンプ: running · 4m12s。OSC 133 の状態遷移からの経過）
        let state_elapsed = self
            .terminals
            .get(&pane_id)
            .and_then(|s| s.command_state_since())
            .map(|t| format_state_elapsed(t.elapsed()));
        // ペイン番号（カンプ: 17×17 バッジ。タブ内の表示順で 1 始まり）
        let pane_index = self
            .workspace
            .active_tab()
            .tree()
            .panes()
            .iter()
            .position(|p| p.id() == pane_id)
            .map(|i| i + 1)
            .unwrap_or(0);
        // cwd チップ（カンプ: ~/projects/tako。クリックでコピー)
        let cwd_display = self.terminals.get(&pane_id).and_then(|s| s.cwd()).map(|p| {
            let full = p.to_string_lossy().to_string();
            let home = std::env::var("HOME").unwrap_or_default();
            let short = if !home.is_empty() && full.starts_with(&home) {
                format!("~{}", &full[home.len()..])
            } else {
                full.clone()
            };
            (short, full)
        });
        // master: 子ワーカー一覧（spawned_by チェーン。全タブ走査）
        let workers: Vec<WorkerRow> = if is_master {
            self.workspace
                .tabs()
                .iter()
                .flat_map(|t| t.tree().panes())
                .filter(|p| p.spawned_by() == Some(pane_id))
                .map(|p| {
                    let name = p
                        .role()
                        .or_else(|| p.title())
                        .unwrap_or("worker")
                        .to_string();
                    let subtitle = p
                        .title()
                        .map(str::to_string)
                        .filter(|t| *t != name)
                        .unwrap_or_default();
                    let st = self
                        .terminals
                        .get(&p.id())
                        .map(|s| s.command_state())
                        .unwrap_or(tako_core::CommandState::Unknown);
                    WorkerRow {
                        pane: p.id(),
                        name,
                        subtitle,
                        state: st,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        // worker: 親 master へのリンク（↳ master）
        let parent_master = pane_info.and_then(|p| p.spawned_by()).and_then(|parent| {
            self.workspace
                .tabs()
                .iter()
                .flat_map(|t| t.tree().panes())
                .find(|p| p.id() == parent)
                .map(|p| {
                    (
                        parent,
                        p.role()
                            .or_else(|| p.title())
                            .unwrap_or("master")
                            .to_string(),
                    )
                })
        });
        let workers_menu_open = self.workers_menu_open == Some(pane_id);
        // #185 見切れ解消: 幅に応じた段階的省略。× は最後まで必ず残す
        let header_w = f32::from(area.size.width);
        let hv = tako_core::HeaderVisibility::from_width(header_w);

        // スクロールバー（FR-2.5.13）: iTerm2 流にスクロール中だけ表示 → フェードアウト。
        // バックエンドペインは tmux 側（ネスト先含む）の位置・履歴を表示する
        let scrollbar = self.scrollbar_overlay(pane_id, area);

        // 提案チップ（FR-2.4.3）。このペインの先頭 1 件だけ下端に出す（残りは閉じたら順に）
        let suggestion = self
            .port_suggestions
            .iter()
            .find(|s| s.pane == pane_id)
            .map(|s| (s.port, s.process.clone()));

        // ミラー方式（#159）では copy-mode に入らないためカーソルを隠す必要はない
        // （ライブ画面部分のカーソルはスクロール中も見えるのが iTerm2 と同じ挙動）
        let lines = self.terminal_screen_lines(pane_id, true);
        // サブラインスクロールの描画シフト（fract 行ぶん行スタック全体を上へずらす。#159）。
        // バックエンドペインはミラー位置の端数、直接ペインはセッションの端数
        let subline_fract = self
            .scroll_ctls
            .get(&pane_id)
            .and_then(|c| c.mirror.as_ref())
            .map(|m| {
                let pos = m.effective_position();
                pos.ceil() - pos
            })
            .or_else(|| {
                self.terminals
                    .get(&pane_id)
                    .map(|s| s.scroll_subline_fract())
            })
            .unwrap_or(0.0);
        let subline_shift = subline_fract * f32::from(cell.height);
        let has_link_hover = self
            .hovered_link
            .as_ref()
            .is_some_and(|h| h.pane == pane_id);

        div()
            .id(("pane", pane_id.as_u64()))
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .bg(rgba(theme.background))
            .border(px(PANE_BORDER))
            .rounded(px(9.0))
            .border_color(if is_failed {
                // カンプ: 失敗ペインは赤枠で明確に（rgba(243,139,168,0.55)）
                hsla_alpha(theme.red, 0.55)
            } else if focused {
                hsla(theme.accent)
            } else {
                hsla(theme.border_default)
            })
            .when(is_failed, |d| {
                d.shadow(vec![BoxShadow {
                    color: hsla_alpha(theme.red, 0.12),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(1.),
                    inset: false,
                }])
            })
            .when(focused && !is_failed, |d| {
                d.shadow(vec![
                    BoxShadow {
                        color: hsla_alpha(theme.accent, 0.22),
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
                    .gap(px(8.0))
                    .px(px(10.0))
                    .bg(rgba(if is_failed {
                        // カンプ: 失敗ペインのヘッダは赤みの面（#241b26）
                        theme.danger_header
                    } else if focused {
                        theme.surface_2
                    } else {
                        theme.surface_0
                    }))
                    .border_b_1()
                    .border_color(if is_failed {
                        hsla_alpha(theme.red, 0.25)
                    } else if focused {
                        hsla(theme.border_default)
                    } else {
                        hsla(theme.border_subtle)
                    })
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
                    // #185: ペインヘッダ右クリックメニュー
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.pane_context_menu = Some(PaneContextMenu {
                                pane: pane_id,
                                kind: PaneContextKind::Terminal,
                                position: event.position,
                            });
                            cx.notify();
                        }),
                    )
                    // #185: 左コンテナ（情報要素、flex_1 + overflow_hidden）
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            // ペイン番号バッジ（カンプ: 17x17 / radius 5 / mono 10px 700）
                            .when(hv.badge && pane_index > 0, |d| {
                                let (badge_bg, badge_fg) = if is_failed {
                                    (rgba_alpha(theme.red, 0.16), theme.red)
                                } else if focused {
                                    (rgba_alpha(theme.accent, 0.16), theme.accent)
                                } else {
                                    (rgba_alpha(theme.accent, 0.10), theme.accent_muted)
                                };
                                d.child(
                                    div()
                                        .w(px(17.0))
                                        .h(px(17.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.0))
                                        .bg(badge_bg)
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(10.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(hsla(badge_fg))
                                        .child(SharedString::from(pane_index.to_string())),
                                )
                            })
                            // ペイン名（カンプ: mono 12px 600）
                            .when(hv.title, |d| {
                                d.child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_family(theme.font_family.clone())
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(if focused {
                                            hsla(theme.foreground)
                                        } else {
                                            hsla(theme.text_secondary)
                                        })
                                        .child(SharedString::from(truncate(&title_label, 40))),
                                )
                            })
                            // role ラベル（カンプ: 素のテキスト 9.5px 600 tracking 0.06em）
                            .when(hv.role, |d| {
                                d.children(role_label.map(|(label, color)| {
                                    div()
                                        .flex_none()
                                        .text_size(px(9.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(color))
                                        .child(SharedString::from(label))
                                }))
                            })
                            // master: 「N workers ▾」ドロップダウンボタン
                            .when(
                                hv.workers_dropdown && is_master && !workers.is_empty(),
                                |d| {
                                    let n = workers.len();
                                    d.child(
                                        div()
                                            .id(("pane-workers", pane_id.as_u64()))
                                            .flex()
                                            .flex_none()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(4.0))
                                            .px(px(8.0))
                                            .py(px(3.0))
                                            .rounded(px(6.0))
                                            .bg(rgba(theme.chip_surface))
                                            .border_1()
                                            .border_color(hsla(theme.border_heavy))
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(hsla(theme.text_tertiary))
                                            .cursor_pointer()
                                            .hover(|d| {
                                                d.text_color(hsla(theme.foreground))
                                                    .border_color(hsla(theme.text_overlay))
                                            })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                    cx.stop_propagation()
                                                }),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.workers_menu_open =
                                                    if this.workers_menu_open == Some(pane_id) {
                                                        None
                                                    } else {
                                                        Some(pane_id)
                                                    };
                                                cx.notify();
                                            }))
                                            .child(SharedString::from(format!("{n} workers")))
                                            .child(
                                                svg()
                                                    .path(crate::file_icons::ui_icon::CHEVRON_DOWN)
                                                    .w(px(9.0))
                                                    .h(px(9.0))
                                                    .text_color(hsla(theme.text_tertiary)),
                                            ),
                                    )
                                },
                            )
                            // worker: 「↳ master」親リンク
                            .when(hv.parent_link, |d| {
                                d.children(parent_master.clone().map(|(parent_id, parent_name)| {
                                    div()
                                        .id(("pane-parent", pane_id.as_u64()))
                                        .flex_none()
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(10.5))
                                        .text_color(hsla(theme.text_muted))
                                        .cursor_pointer()
                                        .hover(|d| d.text_color(hsla(theme.accent)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.jump_to_pane(parent_id, cx);
                                        }))
                                        .child(SharedString::from(format!(
                                            "\u{21B3} {}",
                                            truncate(&parent_name, 16)
                                        )))
                                }))
                            })
                            // 状態表示（ドット + running · 4m12s / fail_x + failed）
                            .when(hv.state && is_failed, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_none()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(5.0))
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.red))
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::FAIL_X)
                                                .w(px(11.0))
                                                .h(px(11.0))
                                                .text_color(hsla(theme.red)),
                                        )
                                        .child("failed"),
                                )
                            })
                            .when(hv.state && !is_failed, |d| {
                                d.children(state_dot.map(|color| {
                                    let label = match (state_label, &state_elapsed) {
                                        (Some("running"), Some(el)) if hv.state_elapsed => {
                                            format!("running \u{00B7} {el}")
                                        }
                                        (Some("running"), _) => "running".to_string(),
                                        (Some(l), _) => l.to_string(),
                                        (None, _) => String::new(),
                                    };
                                    div()
                                        .flex()
                                        .flex_none()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(5.0))
                                        .text_size(px(10.5))
                                        .text_color(if state_label == Some("running") {
                                            hsla(theme.accent)
                                        } else {
                                            hsla(theme.text_muted)
                                        })
                                        .child(
                                            div()
                                                .w(px(6.0))
                                                .h(px(6.0))
                                                .rounded_full()
                                                .bg(hsla(color)),
                                        )
                                        .child(SharedString::from(label))
                                }))
                            })
                            // cwd チップ（カンプ: mono 10.5 / chip 面 / クリックでコピー）
                            .when(hv.cwd_chip, |d| {
                                d.children(cwd_display.map(|(short, full)| {
                                    div()
                                        .id(("pane-cwd", pane_id.as_u64()))
                                        .flex()
                                        .flex_none()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(4.0))
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .rounded(px(5.0))
                                        .bg(rgba(theme.chip_surface))
                                        .border_1()
                                        .border_color(hsla(theme.border_subtle))
                                        .font_family(theme.font_family.clone())
                                        .text_size(px(10.5))
                                        .text_color(hsla(theme.text_muted))
                                        .cursor_pointer()
                                        .hover(|d| {
                                            d.text_color(hsla(theme.text_tertiary))
                                                .border_color(hsla(theme.border_heavy))
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.stop_propagation();
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                full.clone(),
                                            ));
                                        }))
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::FOLDER)
                                                .w(px(10.0))
                                                .h(px(10.0))
                                                .text_color(hsla(theme.text_muted)),
                                        )
                                        .child(SharedString::from(truncate(&short, 28)))
                                }))
                            })
                            // ターミナル情報（シェル名 · cols x rows）
                            .when(hv.shell_info, |d| {
                                let shell_name = self
                                    .terminals
                                    .get(&pane_id)
                                    .and_then(|s| s.title())
                                    .unwrap_or("zsh");
                                let shell_short =
                                    shell_name.rsplit('/').next().unwrap_or(shell_name);
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.5))
                                        .font_family(theme.font_family.clone())
                                        .text_color(hsla(theme.tab_inactive_foreground))
                                        .child(SharedString::from(format!(
                                            "{shell_short} \u{00B7} {cols}\u{00D7}{rows}"
                                        ))),
                                )
                            }),
                    )
                    // #185: 右コンテナ（操作ボタン、flex_none — 常に表示）
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            // failed: 再実行ボタン
                            .when(is_failed && !hv.more_menu, |d| {
                                d.child(
                                    div()
                                        .id(("pane-retry", pane_id.as_u64()))
                                        .flex()
                                        .flex_none()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(4.0))
                                        .px(px(9.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .border_1()
                                        .border_color(hsla_alpha(theme.red, 0.35))
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hsla(theme.red))
                                        .cursor_pointer()
                                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.10)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.retry_last_command(pane_id, cx);
                                        }))
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::RETRY)
                                                .w(px(11.0))
                                                .h(px(11.0))
                                                .text_color(hsla(theme.red)),
                                        )
                                        .child("再実行"),
                                )
                            })
                            // split ボタン（カンプ: 13px SVG）
                            .when(hv.split_button, |d| {
                                d.child(
                                    div()
                                        .id(("pane-split", pane_id.as_u64()))
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .hover(|d| d.bg(rgba(theme.surface_highlight)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.split_pane_button(
                                                pane_id,
                                                SplitDirection::Right,
                                                cx,
                                            );
                                        }))
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::SPLIT)
                                                .w(px(13.0))
                                                .h(px(13.0))
                                                .text_color(hsla(theme.text_muted)),
                                        ),
                                )
                            })
                            // バックグラウンドボタン
                            .when(hv.bg_button, |d| {
                                d.child(
                                    div()
                                        .id(("pane-bg", pane_id.as_u64()))
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .hover(|d| d.bg(rgba(theme.surface_highlight)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.background_pane_button(pane_id, cx);
                                        }))
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::MINUS)
                                                .w(px(13.0))
                                                .h(px(13.0))
                                                .text_color(hsla(theme.text_muted)),
                                        ),
                                )
                            })
                            // #229: 狭幅時は「...」メニューに bg/close を集約
                            .when(hv.more_menu, |d| {
                                d.child(
                                    div()
                                        .id(("pane-more", pane_id.as_u64()))
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .hover(|d| d.bg(rgba(theme.surface_highlight)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                move |this, event: &MouseDownEvent, _, cx| {
                                                    cx.stop_propagation();
                                                    this.pane_context_menu =
                                                        Some(PaneContextMenu {
                                                            pane: pane_id,
                                                            kind: PaneContextKind::Terminal,
                                                            position: event.position,
                                                        });
                                                    cx.notify();
                                                },
                                            ),
                                        )
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::MORE)
                                                .w(px(13.0))
                                                .h(px(13.0))
                                                .text_color(hsla(theme.text_muted)),
                                        ),
                                )
                            })
                            // 閉じるボタン（more_menu でないとき表示。hover で赤）
                            .when(hv.close_button, |d| {
                                d.child(
                                    div()
                                        .id(("pane-close", pane_id.as_u64()))
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(5.0))
                                        .cursor_pointer()
                                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.25)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _: &MouseDownEvent, _, cx| {
                                                cx.stop_propagation()
                                            }),
                                        )
                                        .on_click(cx.listener(
                                            move |this, event: &gpui::ClickEvent, _, cx| {
                                                cx.stop_propagation();
                                                this.close_pane_with_confirm(
                                                    pane_id,
                                                    event.modifiers().platform,
                                                    cx,
                                                );
                                            },
                                        ))
                                        .child(
                                            svg()
                                                .path(crate::file_icons::ui_icon::CLOSE)
                                                .w(px(13.0))
                                                .h(px(13.0))
                                                .text_color(hsla(theme.text_muted)),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                // テキスト領域: サブラインスクロール（#159）のため行スタックを
                // absolute 配置し、fract 行ぶん上へずらして描画する（overflow_hidden で
                // 上下端は部分行として見切れる = ピクセル単位のスムーススクロール）
                div()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    .when(has_link_hover, |d| d.cursor(CursorStyle::PointingHand))
                    .child(
                        div()
                            .absolute()
                            .left(px(PANE_PADDING))
                            .right(px(PANE_PADDING))
                            .top(px(PANE_PADDING - subline_shift))
                            .flex()
                            .flex_col()
                            .children(lines),
                    ),
            )
            .children(scrollbar.map(|(top, thumb_h, track_h, alpha, emphasized)| {
                // オーバーレイスクロールバー（macOS 慣行 #159）: スクロール中に表示 →
                // 停止 1 秒でフェードアウト。ホバー / ドラッグ中は表示を維持し、
                // トラックをうっすら敷いてサムを太く・濃くする
                div()
                    .id(("scrollbar", pane_id.as_u64()))
                    .absolute()
                    .top(px(PANE_TITLE_BAR))
                    .right(px(0.0))
                    .w(px(SCROLLBAR_WIDTH))
                    .h(px(track_h))
                    .occlude() // 下のペインへの選択開始を防ぐ
                    .when(emphasized, |d| {
                        d.bg(rgba_alpha(theme.surface_highlight, alpha * 0.5))
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.hovered_scrollbar = Some(pane_id);
                        } else if this.hovered_scrollbar == Some(pane_id) {
                            this.hovered_scrollbar = None;
                            // 離脱時からフェード猶予を数え直す
                            this.mark_scroll_activity(pane_id, cx);
                        }
                        cx.notify();
                    }))
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
                            .w(px(if emphasized {
                                SCROLLBAR_WIDTH - 2.0
                            } else {
                                SCROLLBAR_WIDTH - 4.0
                            }))
                            .h(px(thumb_h))
                            .rounded_sm()
                            .bg(rgba_alpha(
                                theme.tab_inactive_foreground,
                                alpha * if emphasized { 0.7 } else { 0.45 },
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
            // 子ワーカードロップダウン（カンプ: w282 / radius 9。ヘッダ下に絶対配置）
            // ターミナルテキストエリアより後に描画し、背後が透けないようにする（#341）
            .when(workers_menu_open, |d| {
                d.child(self.render_workers_menu(pane_id, &workers, cx))
            })
            .into_any_element()
    }
}

/// GPUI が描画に使った TextLayout から、ウィンドウ座標を論理行と UTF-8 byte index へ戻す。
/// bounds はスクロール後のウィンドウ座標を含むため、別途 padding / scroll / HiDPI 補正を
/// 重ねず、描画と逆写像を同じデータに揃える。
fn preview_text_layout_hit_test(
    layouts: &[Option<TextLayout>],
    texts: &[String],
    position: Point<Pixels>,
) -> Option<(usize, usize)> {
    let mut last_text_line = None;
    for (i, layout) in layouts.iter().enumerate() {
        let Some(layout) = layout else {
            continue;
        };
        let line_text = texts.get(i).map(String::as_str).unwrap_or("");
        let bounds = layout.bounds();
        if position.y < bounds.top() {
            return Some((i, 0));
        }
        if position.y <= bounds.bottom() {
            // GPUI の index_for_position は glyph の内側判定で、キャレット境界ちょうどでは
            // 直前 glyph の開始 byte を返す。raw と次の UTF-8 境界それぞれの実キャレット
            // 座標を比較し、クリック点に近い挿入位置へ丸める。
            let raw = layout
                .index_for_position(position)
                .unwrap_or_else(|nearest| nearest)
                .min(line_text.len());
            let before = snap_to_char_boundary(line_text, raw);
            let after = line_text[before..]
                .chars()
                .next()
                .map(|ch| before + ch.len_utf8())
                .unwrap_or(before);
            let distance = |byte| {
                layout
                    .position_for_index(byte)
                    .map(|caret| {
                        let dx = f32::from(caret.x - position.x);
                        let dy = f32::from(caret.y - position.y);
                        dx * dx + dy * dy
                    })
                    .unwrap_or(f32::INFINITY)
            };
            let byte_offset = if distance(after) < distance(before) {
                after
            } else {
                before
            };
            return Some((i, byte_offset));
        }
        last_text_line = Some(i);
    }
    last_text_line.map(|i| (i, texts.get(i).map_or(0, String::len)))
}

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

/// GPUI の StyledText::compute_runs は highlight 範囲がソート済み・非重複であることを
/// 前提としている。構文ハイライトと選択ハイライトが重なると compute_runs が text.len()
/// を超えるランを生成し panic する。この関数で重複を解消する。
fn merge_highlights(
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    if highlights.len() <= 1 {
        return highlights;
    }
    let mut boundaries = std::collections::BTreeSet::new();
    for (range, _) in &highlights {
        boundaries.insert(range.start);
        boundaries.insert(range.end);
    }
    let boundaries: Vec<usize> = boundaries.into_iter().collect();
    let mut result = Vec::with_capacity(boundaries.len());
    for w in boundaries.windows(2) {
        let seg_start = w[0];
        let seg_end = w[1];
        if seg_start >= seg_end {
            continue;
        }
        let mut merged: Option<HighlightStyle> = None;
        for (range, style) in &highlights {
            if range.start <= seg_start && seg_end <= range.end {
                merged = Some(match merged {
                    None => *style,
                    Some(m) => m.highlight(*style),
                });
            }
        }
        if let Some(style) = merged {
            result.push((seg_start..seg_end, style));
        }
    }
    result
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
/// dispatch 直後（IPC リクエストループ内）で実行する。
/// ControlHost は blanket impl で自動導出される（全サブトレイトを実装すれば成立）
impl WorkspaceHost for TakoApp {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
}

impl SessionHost for TakoApp {
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
        self.prompt_flows.push(PromptFlow::new(pane, prompt, true));
    }

    fn queue_send_flow(&mut self, pane: PaneId, text: String) {
        self.prompt_flows.push(PromptFlow::new(pane, text, false));
    }

    fn queue_enter_flow(&mut self, pane: PaneId) {
        self.prompt_flows.push(PromptFlow::new_enter_only(pane));
    }

    fn detach_session(&mut self, pane: PaneId) {
        // ペインログの最終フラッシュ（Issue #112 B。CLI / MCP の close はこの経路で
        // セッションを破棄するため、terminals から外す前に可視画面を書き残す）
        if let Some(data) = self.pane_log_close_data(pane) {
            self.apply_pane_log_close(pane, data, CloseReason::Explicit);
        }
        self.terminals.remove(&pane);
        self.previews.remove(&pane);
        self.preview_edits.remove(&pane);
        self.remove_preview_image_cache(pane);
        self.preview_views.remove(&pane);
        self.preview_scroll_handles.remove(&pane);
        self.video_players.remove(&pane);
        self.remove_video_frame_cache(pane);
        self.pane_links.remove(&pane);
        self.known_failed.remove(&pane);
        self.sync_preview_watches();
        self.dock_webview_of(pane);
        self.scroll_accum.remove(&pane);
        self.scroll_ctls.remove(&pane);
        self.drop_tmux_view_session(pane);
        self.drop_backend_session(pane);
    }

    fn reattach_backgrounded(&mut self, pane: PaneId) {
        // ターミナル: セッションは terminals HashMap に残っている。再描画のみ必要。
        // プレビュー: 退避中に停止していたファイル監視を再開し、退避中の変更を反映する（#230）
        if let Some(state) = self.previews.get(&pane) {
            let path = state.path.clone();
            let mode = state.mode;
            self.sync_preview_watches();
            if preview::live_reload_supported(mode) {
                self.pending_preview_loads.push((pane, path, mode));
            }
        }
    }
}

impl TmuxHost for TakoApp {
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
        // 明示操作（tako tmux cleanup / MCP）は従来どおり猶予なし
        self.cleanup_orphan_tmux_with(None)
    }

    fn tmux_tab_collapsed(&self, tab: TabId) -> bool {
        self.collapsed_tmux_tabs.contains(&tab)
    }

    fn set_tmux_tab_collapsed(&mut self, tab: TabId, collapsed: Option<bool>) {
        self.set_tmux_collapsed(tab, collapsed);
    }

    fn tmux_persist_enabled(&self) -> bool {
        self.tmux_persist
    }

    fn set_tmux_persist(&mut self, enabled: bool) {
        if self.secondary {
            eprintln!(
                "warning: セカンダリモードのため persist 切替を無視（プライマリ側で操作してください）"
            );
            return;
        }
        self.tmux_persist = enabled;
        if enabled && tako_core::tmux_backend::available() {
            tako_core::tmux_backend::sync_conf(&tako_core::tmux_backend::socket_name());
        }
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.tmux_persist = enabled;
            if let Err(e) = tako_control::settings::save(&settings) {
                eprintln!("warning: 設定を保存できない: {e}");
            }
            if !enabled {
                tako_control::layout::remove();
                self.last_saved_layout = None;
                persist_diag("layout.json 削除: persist を OFF に切替（次回は空で起動）");
            }
        }
    }

    fn backend_session(&self, pane: PaneId) -> Option<String> {
        self.backend_sessions.get(&pane).cloned()
    }

    fn is_mirror_scroll_pane(&self, pane: PaneId) -> bool {
        self.mirror_scroll_pane(pane)
    }

    fn backend_windows(&self, pane: PaneId) -> Option<Vec<tako_core::TmuxWindow>> {
        self.backend_windows.get(&pane).cloned()
    }

    fn backend_scroll_view(
        &mut self,
        pane: PaneId,
        to: Option<usize>,
        delta: Option<i32>,
    ) -> Option<(usize, usize)> {
        let source = self.mirror_source(pane)?;
        let ctl = self.scroll_ctls.entry(pane).or_default();
        ctl.last_activity = std::time::Instant::now();
        if ctl.target.is_none() {
            ctl.target = Some(match source {
                MirrorSource::Backend(backend) => {
                    let socket = tako_core::tmux_backend::socket_name();
                    tako_core::scroll::resolve_target(&socket, &backend, &[None, Some(&socket)])
                }
                MirrorSource::Fixed(t) => t,
            });
        }
        let target = ctl.target.as_ref().expect("直前に解決済み");
        if let Some(s) = tako_core::scroll_mirror::history_state(target) {
            ctl.known_history = s.history;
            ctl.wants_mouse = Some(s.mouse);
            ctl.wants_sgr = s.sgr;
            if let Some(m) = ctl.mirror.as_mut() {
                m.total_history = m.total_history.max(s.history);
            }
        }
        let history = ctl.known_history;
        let current = ctl
            .mirror
            .as_ref()
            .map(|m| m.position)
            .unwrap_or(ctl.pending_rows);
        let goal = match (to, delta) {
            (Some(t), None) => t as f32,
            (None, Some(d)) => current + d as f32,
            _ => current,
        }
        .clamp(0.0, history as f32);
        match ctl.mirror.as_mut() {
            Some(m) => {
                m.position = goal;
                if m.position <= 0.0 {
                    ctl.mirror = None;
                }
                ctl.pending_rows = 0.0;
            }
            None => {
                ctl.pending_rows = goal;
            }
        }
        Some((goal.round() as usize, history))
    }
}

impl UiStateHost for TakoApp {
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

    fn confirm_close_enabled(&self) -> bool {
        self.confirm_close
    }

    fn set_confirm_close(&mut self, enabled: bool) {
        self.confirm_close = enabled;
    }

    fn theme_mode(&self) -> tako_core::theme::ThemeMode {
        self.theme.mode
    }

    fn set_theme_mode(&mut self, mode: tako_core::theme::ThemeMode) {
        if self.theme.mode == mode {
            return;
        }
        self.theme = Theme::for_mode(mode);
    }

    fn limit_service(&self) -> tako_core::LimitService {
        self.limit_service
    }

    fn set_limit_service(&mut self, service: tako_core::LimitService) {
        self.limit_service = service;
    }

    fn panel_state(&self) -> (bool, f32, tako_control::protocol::PanelViewWire) {
        let view = match self.panel_view {
            PanelView::Tmux => tako_control::protocol::PanelViewWire::Tmux,
            PanelView::Orch => tako_control::protocol::PanelViewWire::Orch,
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
                tako_control::protocol::PanelViewWire::Orch => PanelView::Orch,
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

    fn sidebar_width(&self) -> f32 {
        self.sidebar_width
    }

    fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar_width = width.clamp(SIDEBAR_MIN_WIDTH, 600.0);
    }

    fn set_filetree(&mut self, visible: bool) {
        if self.filetree.visible != visible {
            self.toggle_filetree();
        }
    }

    fn sync_filetree(&mut self) {
        self.sync_filetree_roots();
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
}

impl PreviewHost for TakoApp {
    fn preview_reload_enabled(&self) -> bool {
        self.preview_reload.enabled()
    }

    fn set_preview_reload(&mut self, enabled: bool) {
        // セルフテスト中はユーザー設定を汚さない。
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.preview_live_reload = enabled;
            if let Err(e) = tako_control::settings::save(&settings) {
                eprintln!("warning: 設定を保存できない: {e}");
            }
        }
        // OS 監視の初期化に失敗したプロセスで ON を報告しない。
        let effective = enabled && self.preview_file_watcher.is_some();
        if self.preview_reload.set_enabled(effective) {
            self.sync_preview_watches();
        }
    }

    fn preview_cache_stats(&self) -> tako_core::PreviewCacheStats {
        tako_core::PreviewCacheStats {
            max_bytes: self.preview_image_lru.budget_bytes(),
            used_bytes: self.preview_image_lru.used_bytes(),
            entries: self.preview_image_lru.len(),
        }
    }

    fn set_preview_cache_budget(&mut self, max_bytes: u64) {
        let evicted = self.preview_image_lru.set_budget_bytes(max_bytes);
        self.evict_preview_image_keys(evicted);
        // セルフテスト中はユーザー設定を汚さない。
        if std::env::var_os("TAKO_SELF_TEST").is_none() {
            let mut settings = tako_control::settings::load();
            settings.preview_cache_max_mb = max_bytes / 1024 / 1024;
            if let Err(e) = tako_control::settings::save(&settings) {
                eprintln!("warning: 設定を保存できない: {e}");
            }
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

    fn preview_view_state(&self, pane: PaneId) -> Option<tako_core::PreviewViewState> {
        let preview = self.previews.get(&pane)?;
        if !matches!(
            preview.mode,
            preview::PreviewMode::Pdf | preview::PreviewMode::Image
        ) {
            return None;
        }
        let mut state = self.preview_views.get(&pane).copied().unwrap_or_default();
        if let Some(handle) = self.preview_scroll_handles.get(&pane) {
            let offset = handle.offset();
            state.pan_x = -f32::from(offset.x);
            state.pan_y = -f32::from(offset.y);
            if preview.mode == preview::PreviewMode::Pdf && handle.bounds_for_item(0).is_some() {
                state.page = handle.top_item() + 1;
            }
        }
        Some(state)
    }

    fn update_preview_view(
        &mut self,
        pane: PaneId,
        update: tako_core::PreviewViewUpdate,
    ) -> Result<tako_core::PreviewViewState, String> {
        let preview = self
            .previews
            .get(&pane)
            .ok_or_else(|| "プレビューペインではない".to_string())?;
        let (is_pdf, total_pages) = match &preview.content {
            preview::PreviewContent::Pdf(data) => (true, data.total_pages),
            preview::PreviewContent::Loading if preview.mode == preview::PreviewMode::Pdf => {
                (true, usize::MAX)
            }
            preview::PreviewContent::Image(_) => (false, 1),
            _ if preview.mode == preview::PreviewMode::Image => (false, 1),
            _ => return Err("ズーム操作は PDF・画像プレビューだけに対応する".into()),
        };
        let mut state = self.preview_view_state(pane).unwrap_or_default();
        state.apply(update)?;
        if state.page > total_pages {
            return Err(format!(
                "ページ範囲外: {}（全 {total_pages} ページ）",
                state.page
            ));
        }
        if !is_pdf && state.page != 1 {
            return Err("画像プレビューの page は 1 だけ指定できる".into());
        }

        let handle = self.preview_scroll_handles.entry(pane).or_default().clone();
        let restore_page = update.page.or_else(|| {
            matches!(update.zoom, Some(tako_core::PreviewZoomCommand::Reset)).then_some(state.page)
        });
        if let Some(page) = restore_page {
            handle.scroll_to_top_of_item(page - 1);
            state.pan_y = 0.0;
            handle.set_offset(point(px(-state.pan_x), handle.offset().y));
        } else {
            handle.set_offset(point(px(-state.pan_x), px(-state.pan_y)));
        }
        self.preview_views.insert(pane, state);
        Ok(state)
    }

    fn preview_outline(&self, pane: PaneId) -> Option<tako_core::PreviewOutline> {
        let preview = self.previews.get(&pane)?;
        matches!(
            preview.mode,
            preview::PreviewMode::Markdown | preview::PreviewMode::Pdf
        )
        .then(|| (*preview.outline).clone())
    }

    fn navigate_preview_outline(
        &mut self,
        pane: PaneId,
        item: usize,
    ) -> Result<tako_core::PreviewOutlineTarget, String> {
        let preview = self
            .previews
            .get(&pane)
            .ok_or_else(|| "プレビューペインではない".to_string())?;
        let target = preview.outline.target(item)?;
        let handle = self.preview_scroll_handles.entry(pane).or_default().clone();
        match target {
            tako_core::PreviewOutlineTarget::MarkdownBlock { block } => {
                if preview.mode != preview::PreviewMode::Markdown {
                    return Err("Markdown アウトラインの対象ではない".into());
                }
                handle.scroll_to_top_of_item(block);
            }
            tako_core::PreviewOutlineTarget::PdfPage { page } => {
                let preview::PreviewContent::Pdf(data) = &preview.content else {
                    return Err("PDF アウトラインの対象ではない".into());
                };
                if page == 0 || page > data.total_pages {
                    return Err(format!(
                        "ページ範囲外: {page}（全 {} ページ）",
                        data.total_pages
                    ));
                }
                handle.scroll_to_top_of_item(page - 1);
                let mut view = self.preview_views.get(&pane).copied().unwrap_or_default();
                view.page = page;
                view.pan_y = 0.0;
                self.preview_views.insert(pane, view);
            }
        }
        Ok(target)
    }

    fn preview_pdf_links(&self, pane: PaneId) -> Option<tako_core::PdfLinks> {
        let preview = self.previews.get(&pane)?;
        let data = match &preview.content {
            preview::PreviewContent::Pdf(data) => data,
            _ => return None,
        };
        Some((*data.links).clone())
    }

    fn follow_preview_pdf_link(
        &mut self,
        pane: PaneId,
        index: usize,
    ) -> Result<serde_json::Value, String> {
        let preview = self
            .previews
            .get(&pane)
            .ok_or_else(|| "プレビューペインではない".to_string())?;
        let data = match &preview.content {
            preview::PreviewContent::Pdf(data) => data,
            _ => return Err("PDF プレビューではない".into()),
        };
        let link = data
            .links
            .links
            .get(index)
            .ok_or_else(|| format!("リンクインデックス範囲外: {index}"))?;
        match &link.target {
            tako_core::PdfLinkTarget::Url { url } => {
                let _ = std::process::Command::new("open").arg(url).spawn();
                Ok(serde_json::json!({
                    "pane": pane.as_u64(),
                    "action": "opened_url",
                    "url": url,
                }))
            }
            tako_core::PdfLinkTarget::Page { page } => {
                let page = *page;
                let handle = self.preview_scroll_handles.entry(pane).or_default().clone();
                handle.scroll_to_top_of_item(page - 1);
                let mut view = self.preview_views.get(&pane).copied().unwrap_or_default();
                view.page = page;
                view.pan_y = 0.0;
                self.preview_views.insert(pane, view);
                Ok(serde_json::json!({
                    "pane": pane.as_u64(),
                    "action": "jumped_to_page",
                    "page": page,
                }))
            }
        }
    }

    fn video_playback(&mut self, pane: PaneId, action: &str) -> Result<String, String> {
        let player = self
            .video_players
            .get_mut(&pane)
            .ok_or_else(|| "動画プレイヤーが起動していない".to_string())?;
        if let Some(rate_str) = action.strip_prefix("rate:") {
            let rate: f32 = rate_str
                .parse()
                .map_err(|_| format!("不正な速度値: {rate_str}"))?;
            if !(0.1..=4.0).contains(&rate) {
                return Err(format!("速度は 0.1〜4.0 の範囲: {rate}"));
            }
            player.set_rate(rate);
            return Ok(format!("rate:{rate}"));
        }
        match action {
            "play" => player.play(),
            "pause" => player.pause(),
            "toggle" => player.toggle(),
            "mute" => {
                player.muted = true;
                player.set_volume(player.volume);
                return Ok("muted".to_string());
            }
            "unmute" => {
                player.muted = false;
                player.set_volume(player.volume);
                return Ok("unmuted".to_string());
            }
            "toggle_mute" => {
                player.toggle_mute();
                return Ok(if player.muted { "muted" } else { "unmuted" }.to_string());
            }
            "loop_on" => {
                player.looping = true;
                return Ok("loop_on".to_string());
            }
            "loop_off" => {
                player.looping = false;
                return Ok("loop_off".to_string());
            }
            "toggle_loop" => {
                player.toggle_loop();
                return Ok(if player.looping {
                    "loop_on"
                } else {
                    "loop_off"
                }
                .to_string());
            }
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

    fn video_volume(&mut self, pane: PaneId, volume: f64) -> Result<f64, String> {
        let player = self
            .video_players
            .get_mut(&pane)
            .ok_or_else(|| "動画プレイヤーが起動していない".to_string())?;
        player.set_volume(volume as f32);
        Ok(player.volume as f64)
    }

    fn set_preview(
        &mut self,
        pane: PaneId,
        path: &str,
        mode: tako_control::protocol::PreviewModeWire,
    ) -> Result<(), String> {
        if self
            .preview_edits
            .get(&pane)
            .is_some_and(preview::EditState::dirty)
        {
            let message = "未保存の変更があるため別ファイルを開けない（先に保存してください）";
            if let Some(edit) = self.preview_edits.get_mut(&pane) {
                edit.message = Some(message.into());
            }
            return Err(message.into());
        }
        let path = std::path::Path::new(path);
        let mode = preview::PreviewMode::from_wire(mode);
        let state = if matches!(
            mode,
            preview::PreviewMode::Markdown
                | preview::PreviewMode::Pdf
                | preview::PreviewMode::Video
        ) {
            self.pending_preview_loads
                .push((pane, path.to_path_buf(), mode));
            preview::PreviewState::loading(path, mode)
        } else {
            let _span = tako_control::diag::perf_span("preview_load");
            let (state, raw) = preview::load_fast(path, mode);
            if let Some(text) = raw {
                self.pending_highlights
                    .push((pane, path.to_path_buf(), text));
            }
            state
        };
        self.preview_edits.remove(&pane);
        self.preview_selections.remove(&pane);
        self.preview_line_bounds.remove(&pane);
        self.preview_pdf_char_bounds.remove(&pane);
        self.preview_pdf_highlight_paint_count.remove(&pane);
        self.preview_pdf_page_image_bounds.remove(&pane);
        self.preview_text_layouts.remove(&pane);
        self.preview_line_texts.remove(&pane);
        self.remove_preview_image_cache(pane);
        self.pending_pdf_rasters.remove(&pane);
        self.preview_views.remove(&pane);
        self.preview_scroll_handles.remove(&pane);
        if self
            .preview_navigation_panel
            .is_some_and(|(open_pane, _)| open_pane == pane)
        {
            self.preview_navigation_panel = None;
        }
        self.previews.insert(pane, state);
        self.sync_preview_watches();
        Ok(())
    }

    fn preview_edit_state(&self, pane: PaneId) -> Option<(bool, bool)> {
        self.previews.get(&pane)?;
        Some(
            self.preview_edits
                .get(&pane)
                .map(|edit| (edit.editing, edit.dirty()))
                .unwrap_or((false, false)),
        )
    }

    fn set_preview_editing(&mut self, pane: PaneId, enabled: bool) -> Result<(), String> {
        self.set_preview_editing_local(pane, enabled)
    }

    fn apply_preview_text(&mut self, pane: PaneId, text: String) -> Result<(), String> {
        self.apply_preview_text_local(pane, text)
    }

    fn save_preview(&mut self, pane: PaneId) -> Result<(), String> {
        self.save_preview_local(pane)
    }

    fn preview_undo(&mut self, pane: PaneId) -> Result<bool, String> {
        self.preview_undo_local(pane)
    }

    fn preview_redo(&mut self, pane: PaneId) -> Result<bool, String> {
        self.preview_redo_local(pane)
    }

    fn preview_autosave(&self, pane: PaneId) -> Option<bool> {
        self.preview_edits.get(&pane).map(|edit| edit.autosave)
    }

    fn set_preview_autosave(&mut self, pane: PaneId, enabled: bool) -> Result<(), String> {
        let edit = self
            .preview_edits
            .get_mut(&pane)
            .ok_or_else(|| "編集モードを開始していない".to_string())?;
        edit.autosave = enabled;
        Ok(())
    }

    fn preview_search(
        &mut self,
        pane: PaneId,
        query: Option<String>,
        direction: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        self.preview_search_local(pane, query, direction)
    }

    fn preview_replace(
        &mut self,
        pane: PaneId,
        query: &str,
        replacement: &str,
        all: bool,
    ) -> Result<serde_json::Value, String> {
        self.preview_replace_local(pane, query, replacement, all)
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

    fn preview_changelog_state(&self, pane: PaneId) -> Option<bool> {
        if self.previews.contains_key(&pane) {
            Some(self.preview_changelogs.contains_key(&pane))
        } else {
            None
        }
    }

    fn set_preview_changelog(
        &mut self,
        pane: PaneId,
        enabled: bool,
        max_count: usize,
    ) -> Result<serde_json::Value, String> {
        let state = self
            .previews
            .get(&pane)
            .ok_or_else(|| format!("プレビューペインではない: {}", pane.as_u64()))?;
        if !enabled {
            self.preview_changelogs.remove(&pane);
            return Ok(serde_json::json!({
                "pane": pane.as_u64(),
                "changelog": false,
            }));
        }
        let path = state.path.clone();
        let repo = tako_core::git::repo_root(&path);
        let repo = match repo {
            Some(r) => r,
            None => {
                let data = preview::ChangelogData::default();
                self.preview_changelogs.insert(pane, data);
                return Ok(serde_json::json!({
                    "pane": pane.as_u64(),
                    "changelog": true,
                    "commits": 0,
                    "message": "git 管理外のファイル",
                }));
            }
        };
        let rel = path
            .strip_prefix(&repo)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        let commits = tako_core::git::log_file_commits(&repo, &rel, max_count);
        let commit_count = commits.len();
        let entries: Vec<preview::ChangelogEntry> = commits
            .into_iter()
            .map(|c| preview::ChangelogEntry {
                commit: c,
                expanded_diff: None,
            })
            .collect();
        let data = preview::ChangelogData {
            entries,
            repo_root: Some(repo),
            rel_path: Some(rel),
        };
        self.preview_changelogs.insert(pane, data);
        Ok(serde_json::json!({
            "pane": pane.as_u64(),
            "changelog": true,
            "commits": commit_count,
        }))
    }

    fn toggle_changelog_diff(
        &mut self,
        pane: PaneId,
        hash: &str,
    ) -> Result<serde_json::Value, String> {
        let data = self
            .preview_changelogs
            .get_mut(&pane)
            .ok_or("チェンジログビューが有効ではない")?;
        let entry = data
            .entries
            .iter_mut()
            .find(|e| e.commit.hash == hash || e.commit.short_hash == hash)
            .ok_or_else(|| format!("コミット {} が見つからない", hash))?;
        if entry.expanded_diff.is_some() {
            entry.expanded_diff = None;
            Ok(serde_json::json!({
                "pane": pane.as_u64(),
                "hash": hash,
                "expanded": false,
            }))
        } else {
            let repo = data.repo_root.as_deref().ok_or("リポジトリ情報がない")?;
            let rel = data.rel_path.as_deref().ok_or("ファイルパス情報がない")?;
            let hunks = tako_core::git::diff_file_commit(repo, &entry.commit.hash, rel);
            entry.expanded_diff = Some(hunks);
            Ok(serde_json::json!({
                "pane": pane.as_u64(),
                "hash": hash,
                "expanded": true,
            }))
        }
    }
}

impl RemoteHost for TakoApp {
    fn remote_start(
        &mut self,
        port: Option<u16>,
        insecure: bool,
    ) -> Result<serde_json::Value, String> {
        tako_control::remote::spawn_daemon(port, insecure)
    }

    fn remote_stop(&mut self) -> Result<serde_json::Value, String> {
        tako_control::remote::daemon_stop()
    }

    fn remote_status(&self) -> serde_json::Value {
        tako_control::remote::daemon_status()
    }
}

impl WebViewHost for TakoApp {
    fn web_open(&mut self, pane: PaneId, url: &str) -> Result<serde_json::Value, String> {
        let id = self.create_webview(&webview::normalize_url(url))?;
        let e = self
            .webviews
            .iter_mut()
            .find(|e| e.id == id)
            .expect("直前に生成した webview");
        e.pane = Some(pane);
        Ok(serde_json::json!({
            "id": id.as_u64(),
            "pane": pane.as_u64(),
            "url": e.current_url(),
        }))
    }

    fn web_show(&mut self, pane: PaneId, id: u64) -> Result<serde_json::Value, String> {
        let e = self
            .webviews
            .iter_mut()
            .find(|e| e.id.as_u64() == id)
            .ok_or(format!("Web ビュー {id} が見つからない（web list で確認）"))?;
        if e.pane.is_some() {
            return Err(format!("Web ビュー {id} は表示中"));
        }
        e.pane = Some(pane);
        Ok(serde_json::json!({
            "id": id,
            "pane": pane.as_u64(),
            "url": e.current_url(),
        }))
    }

    fn web_list(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.webviews
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "id": e.id.as_u64(),
                        "url": e.current_url(),
                        "title": e.current_title(),
                        "pane": e.pane.map(|p| p.as_u64()),
                        "loading": e.is_loading(),
                    })
                })
                .collect(),
        )
    }

    fn web_target(
        &self,
        id: Option<u64>,
        pane: Option<u64>,
    ) -> Result<(u64, Option<PaneId>), String> {
        if let Some(id) = id {
            let e = self
                .webviews
                .iter()
                .find(|e| e.id.as_u64() == id)
                .ok_or(format!("Web ビュー {id} が見つからない（web list で確認）"))?;
            return Ok((id, e.pane));
        }
        if let Some(raw) = pane {
            let e = self
                .webviews
                .iter()
                .find(|e| e.pane.map(|p| p.as_u64()) == Some(raw))
                .ok_or(format!("ペイン {raw} に Web ビューは表示されていない"))?;
            return Ok((e.id.as_u64(), e.pane));
        }
        let shown: Vec<&webview::WebViewEntry> =
            self.webviews.iter().filter(|e| e.pane.is_some()).collect();
        match shown.len() {
            0 => Err("表示中の Web ビューが無い（id か pane で指定）".into()),
            1 => Ok((shown[0].id.as_u64(), shown[0].pane)),
            n => Err(format!(
                "表示中の Web ビューが {n} 個ある（id か pane で指定）"
            )),
        }
    }

    fn web_destroy(&mut self, id: u64) -> Option<PaneId> {
        let idx = self.webviews.iter().position(|e| e.id.as_u64() == id)?;
        let entry = self.webviews.remove(idx);
        entry.pane
    }

    fn web_navigate(&mut self, id: u64, to: &str) -> Result<serde_json::Value, String> {
        let e = self
            .webviews
            .iter()
            .find(|e| e.id.as_u64() == id)
            .ok_or(format!("Web ビュー {id} が見つからない"))?;
        e.navigate(to)?;
        Ok(serde_json::json!({ "id": id, "navigated": to }))
    }

    fn web_eval(&mut self, id: u64, js: &str) -> Result<serde_json::Value, String> {
        let e = self
            .webviews
            .iter_mut()
            .find(|e| e.id.as_u64() == id)
            .ok_or(format!("Web ビュー {id} が見つからない"))?;
        let token = e.eval(js)?;
        Ok(serde_json::json!({
            "id": id,
            "token": token,
            "hint": "結果は action=eval_result で回収（未完なら pending: true）",
        }))
    }

    fn web_eval_result(&mut self, id: u64, token: u64) -> Result<serde_json::Value, String> {
        let e = self
            .webviews
            .iter()
            .find(|e| e.id.as_u64() == id)
            .ok_or(format!("Web ビュー {id} が見つからない"))?;
        match e.take_eval_result(token) {
            Some(result) => {
                let value: serde_json::Value =
                    serde_json::from_str(&result).unwrap_or(serde_json::Value::String(result));
                Ok(serde_json::json!({ "id": id, "token": token, "result": value }))
            }
            None => Ok(serde_json::json!({ "id": id, "token": token, "pending": true })),
        }
    }

    fn web_read(&self, id: u64) -> Result<serde_json::Value, String> {
        let e = self
            .webviews
            .iter()
            .find(|e| e.id.as_u64() == id)
            .ok_or(format!("Web ビュー {id} が見つからない"))?;
        Ok(serde_json::json!({
            "id": id,
            "url": e.current_url(),
            "title": e.current_title(),
            "loading": e.is_loading(),
            "pane": e.pane.map(|p| p.as_u64()),
        }))
    }
}

impl SystemHost for TakoApp {
    fn is_secondary(&self) -> bool {
        self.secondary
    }

    fn persist_restore_report(&self) -> Option<String> {
        self.restore_report.clone()
    }

    fn recovered_sessions_count(&self) -> usize {
        self.recovered_count
    }

    fn resolve_stale_pane(&self, stale: PaneId) -> Option<PaneId> {
        self.stale_pane_map.get(&stale).copied()
    }

    fn reserve_backend_session(&mut self, pane: PaneId) -> Option<String> {
        if self.tmux_persist && tako_core::tmux_backend::available() {
            Some(
                self.backend_sessions
                    .entry(pane)
                    .or_insert_with(new_backend_session_name)
                    .clone(),
            )
        } else {
            None
        }
    }

    fn update_status(&self) -> serde_json::Value {
        update_checker::update_status_json()
    }
    fn update_check(&self) -> serde_json::Value {
        match update_checker::check_latest() {
            Ok(Some(info)) => serde_json::json!({
                "available": true,
                "version": info.version,
                "download_url": info.download_url,
            }),
            Ok(None) => serde_json::json!({ "available": false }),
            Err(e) => {
                let mut json = serde_json::json!({
                    "available": false,
                    "error": e.to_json(),
                });
                json["error_message"] = serde_json::Value::String(e.to_string());
                json
            }
        }
    }
    fn update_apply(&mut self) -> Result<serde_json::Value, String> {
        let info = update_checker::check_latest()
            .map_err(|e| format!("更新チェックに失敗: {e}"))?
            .ok_or_else(|| "新しいバージョンが見つからない（既に最新版です）".to_string())?;
        update_checker::perform_update(&info)?;
        Ok(serde_json::json!({
            "updated": true,
            "version": info.version,
            "install_method": update_checker::detect_install_method().label(),
        }))
    }
    fn update_apply_zip(&mut self) -> Result<serde_json::Value, String> {
        let info = update_checker::check_latest()
            .map_err(|e| format!("更新チェックに失敗: {e}"))?
            .ok_or_else(|| "新しいバージョンが見つからない（既に最新版です）".to_string())?;
        update_checker::perform_update_zip(&info)?;
        Ok(serde_json::json!({
            "updated": true,
            "version": info.version,
            "install_method": "zip (fallback)",
        }))
    }
    fn update_repair(&mut self) -> Result<serde_json::Value, String> {
        let msg = update_checker::repair_brew()?;
        Ok(serde_json::json!({
            "repaired": true,
            "message": msg,
            "install_method": update_checker::detect_install_method_full().label(),
        }))
    }

    fn pane_log_config(&self) -> tako_core::pane_log::PaneLogConfig {
        self.pane_logs_lock().config()
    }

    fn apply_pane_log_config(&mut self, config: tako_core::pane_log::PaneLogConfig) {
        self.pane_logs_lock().set_config(config);
    }

    fn pane_log_file(&self, pane: PaneId) -> Option<std::path::PathBuf> {
        self.pane_logs_lock().path_of(pane.as_u64())
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
        if self.ime.is_none() {
            let pane = self.ime_target();
            if let Some(edit) = self.preview_edits.get(&pane).filter(|edit| edit.editing) {
                let start = utf16_to_byte_offset(edit.buffer.text(), range_utf16.start);
                let end = utf16_to_byte_offset(edit.buffer.text(), range_utf16.end);
                return edit.buffer.text().get(start..end).map(str::to_string);
            }
        }
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
        let pane = self.ime_target();
        if self.ime.is_none() {
            if let Some(edit) = self.preview_edits.get(&pane).filter(|edit| edit.editing) {
                let range = edit
                    .buffer
                    .selection()
                    .unwrap_or_else(|| edit.buffer.cursor()..edit.buffer.cursor());
                return Some(UTF16Selection {
                    range: byte_to_utf16_offset(edit.buffer.text(), range.start)
                        ..byte_to_utf16_offset(edit.buffer.text(), range.end),
                    reversed: false,
                });
            }
        }
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
            // 検索バー表示中は検索/置換フィールドへ
            if self
                .preview_edits
                .get(&ime.pane)
                .is_some_and(|edit| edit.search_visible)
            {
                if !ime.text.is_empty() {
                    self.insert_search_char(ime.pane, &ime.text);
                }
            } else if let Some(edit) = self
                .preview_edits
                .get_mut(&ime.pane)
                .filter(|edit| edit.editing)
            {
                let p = ime.pane;
                edit.buffer.insert(&ime.text);
                edit.message = None;
                self.refresh_preview_from_editor(p);
                self.schedule_autosave(p);
                self.start_autosave_timer(p, cx);
            } else if let Some(session) = self.terminals.get(&ime.pane) {
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
        // Web dock URL 入力中
        if self.webview_dock_url_focused && !text.is_empty() {
            self.webview_dock_url_input
                .insert_str(self.webview_dock_url_cursor, text);
            self.webview_dock_url_cursor += text.len();
            self.ime = None;
            cx.notify();
            return;
        }
        let pane = self
            .ime
            .as_ref()
            .map(|ime| ime.pane)
            .unwrap_or_else(|| self.focused_pane());
        // 検索バー表示中は入力文字を検索/置換フィールドへ
        if self
            .preview_edits
            .get(&pane)
            .is_some_and(|edit| edit.search_visible)
        {
            if !text.is_empty() {
                self.insert_search_char(pane, text);
            }
            self.ime = None;
            cx.notify();
            return;
        }
        if let Some(edit) = self
            .preview_edits
            .get_mut(&pane)
            .filter(|edit| edit.editing)
        {
            if let Some(range_utf16) = _range_utf16 {
                let start = utf16_to_byte_offset(edit.buffer.text(), range_utf16.start);
                let end = utf16_to_byte_offset(edit.buffer.text(), range_utf16.end);
                edit.buffer.set_cursor(start, false);
                edit.buffer.set_cursor(end, true);
            }
            edit.buffer.insert(text);
            edit.message = None;
            self.ime = None;
            self.refresh_preview_from_editor(pane);
            self.schedule_autosave(pane);
            self.start_autosave_timer(pane, cx);
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
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 変換候補ウィンドウの位置出し。カーソルセル + 範囲先頭までの描画幅。
        // CursorShape::Hidden（claude 等の TUI アプリ）でもカーソル位置は有効なので
        // ime_cursor をフォールバックに使う（#29: 候補ウィンドウが画面左下に出る問題の修正）
        let ime_pane = self.ime_target();
        let origin = self
            .pane_cursor_origin_for_ime(ime_pane, window)
            .unwrap_or(element_bounds.origin);
        let cell = self
            .cell_size_for_pane(ime_pane)
            .unwrap_or(size(px(8.0), px(16.0)));
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

impl TakoApp {
    /// ペインヘッダ / タブの右クリックメニュー描画（#185）
    fn render_pane_context_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let ctx = self.pane_context_menu.as_ref()?;
        let theme = &self.theme;
        let pane_id = ctx.pane;
        let kind = ctx.kind;
        let pos = ctx.position;
        let is_preview = matches!(kind, PaneContextKind::Preview);
        let preview_path = if is_preview {
            self.previews.get(&pane_id).map(|s| s.path.clone())
        } else {
            None
        };
        let cwd = self
            .terminals
            .get(&pane_id)
            .and_then(|s| s.cwd())
            .map(|p| p.to_path_buf());
        let mut items: Vec<(&str, &str)> = Vec::new();
        if is_preview {
            items.push(("copy-path", "パスをコピー"));
            items.push(("reveal", "Finder で表示"));
            items.push(("open-default", "デフォルトアプリで開く"));
            items.push(("sep1", ""));
        } else if cwd.is_some() {
            items.push(("copy-cwd", "cwd をコピー"));
            items.push(("reveal-cwd", "Finder で開く"));
            items.push(("sep1", ""));
        }
        items.push(("split-right", "右に分割"));
        items.push(("split-down", "下に分割"));
        items.push(("sep2", ""));
        items.push(("bg", "バックグラウンドへ"));
        items.push(("close", "閉じる"));

        let pctx_menu_width: f32 = 200.0;
        let pctx_item_height: f32 = 20.0;
        let pctx_sep_height: f32 = 5.0;
        let pctx_padding_y: f32 = 8.0;
        let pctx_menu_height: f32 = items
            .iter()
            .map(|(id, _)| {
                if id.starts_with("sep") {
                    pctx_sep_height
                } else {
                    pctx_item_height
                }
            })
            .sum::<f32>()
            + pctx_padding_y;
        let adjusted = clamp_menu_position(pos, pctx_menu_width, pctx_menu_height, window);

        let menu = div()
            .absolute()
            .left(adjusted.x)
            .top(adjusted.y)
            .w(px(pctx_menu_width))
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
                let preview_path = preview_path.clone();
                let cwd = cwd.clone();
                div()
                    .id(("pctx-item", i as u64))
                    .w_full()
                    .px_2()
                    .py(px(2.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.tab_active_background)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pane_context_menu = None;
                        match id {
                            "copy-path" => {
                                if let Some(p) = &preview_path {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        p.display().to_string(),
                                    ));
                                }
                            }
                            "reveal" => {
                                if let Some(p) = &preview_path {
                                    let _ =
                                        std::process::Command::new("open").arg("-R").arg(p).spawn();
                                }
                            }
                            "open-default" => {
                                if let Some(p) = &preview_path {
                                    let _ = std::process::Command::new("open").arg(p).spawn();
                                }
                            }
                            "copy-cwd" => {
                                if let Some(c) = &cwd {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        c.display().to_string(),
                                    ));
                                }
                            }
                            "reveal-cwd" => {
                                if let Some(c) = &cwd {
                                    let _ = std::process::Command::new("open").arg(c).spawn();
                                }
                            }
                            "split-right" => {
                                this.split_pane_button(pane_id, SplitDirection::Right, cx);
                            }
                            "split-down" => {
                                this.split_pane_button(pane_id, SplitDirection::Down, cx);
                            }
                            "bg" => {
                                this.background_pane_button(pane_id, cx);
                            }
                            "close" => {
                                this.close_pane_button(pane_id, cx);
                            }
                            _ => {}
                        }
                        cx.notify();
                    }))
                    .when(id == "close", |d| d.text_color(hsla(theme.red)))
                    .child(SharedString::from(label.to_string()))
                    .into_any_element()
            }));
        let backdrop = div()
            .id("pctx-backdrop")
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.pane_context_menu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.pane_context_menu = None;
                    cx.notify();
                }),
            )
            .child(menu);
        Some(backdrop.into_any_element())
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

/// メニュー位置���計算（純粋関数・テスト��能）。
/// 見切れる場合はフリップ（メニュー幅/高さ分の引き戻し）、0 未満にはさせない。
fn compute_menu_position(
    x: f32,
    y: f32,
    menu_width: f32,
    menu_height: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> (f32, f32) {
    let rx = if x + menu_width > viewport_width {
        (x - menu_width).max(0.0)
    } else {
        x
    };
    let ry = if y + menu_height > viewport_height {
        (y - menu_height).max(0.0)
    } else {
        y
    };
    (rx, ry)
}

/// コンテキストメニューの位置をウインドウ内にクランプする。
fn clamp_menu_position(
    pos: Point<Pixels>,
    menu_width: f32,
    menu_height: f32,
    window: &Window,
) -> Point<Pixels> {
    let vp = window.viewport_size();
    let (rx, ry) = compute_menu_position(
        f32::from(pos.x),
        f32::from(pos.y),
        menu_width,
        menu_height,
        f32::from(vp.width),
        f32::from(vp.height),
    );
    point(px(rx), px(ry))
}

/// usage テキストから概算トークン数を抽出する（#217 スパークライン用。
/// "48.2k tok" → 48200.0。k/M 単位の数値が無ければ None）
fn parse_tokens_value(s: &str) -> Option<f32> {
    let chars: Vec<char> = s.chars().collect();
    for i in 0..chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                j += 1;
            }
            if j < chars.len() {
                let mult = match chars[j] {
                    'k' | 'K' => 1_000.0,
                    'M' => 1_000_000.0,
                    _ => continue,
                };
                let num: String = chars[start..j].iter().collect();
                if let Ok(v) = num.parse::<f32>() {
                    return Some(v * mult);
                }
            }
        }
    }
    None
}

/// 状態遷移からの経過時間の表示（カンプ: 4m12s 形式。#217）
fn format_state_elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// 子ワーカードロップダウンの 1 行分（#217。render_pane で収集）
#[derive(Clone)]
struct WorkerRow {
    pane: PaneId,
    name: String,
    subtitle: String,
    state: tako_core::CommandState,
}

/// Attention トースト 1 件（#217 カンプ。失敗即知の右下通知）
struct AttentionToast {
    pane: PaneId,
    title: String,
    detail: String,
    at: std::time::Instant,
}

/// ⌘K コマンドパレットの状態（#217 カンプ。ペイン・コマンド検索）
#[derive(Default)]
struct CommandPalette {
    query: String,
    selected: usize,
    mode: PaletteMode,
}

#[derive(Default, Clone)]
enum PaletteMode {
    #[default]
    Normal,
    SshHost(Vec<tako_core::ssh_config::SshHost>),
    RecentItems(Vec<tako_core::recent::RecentEntry>),
}

/// コマンドパレットの候補 1 件（#217）
enum PaletteItem {
    /// ペインへジャンプ（タブ名, 表示名, 状態）
    Pane(PaneId, String, String),
    /// 固定コマンド（表示名, 実行内容の識別子）
    Command(&'static str, &'static str),
    /// SSH ホスト
    SshHost(tako_core::ssh_config::SshHost),
    /// Recent エントリ
    Recent(tako_core::recent::RecentEntry),
}

impl PaletteItem {
    fn label(&self) -> String {
        match self {
            PaletteItem::Pane(_, tab, name) => format!("{tab} \u{203A} {name}"),
            PaletteItem::Command(label, _) => (*label).to_string(),
            PaletteItem::SshHost(h) => {
                let mut s = h.name.clone();
                if let Some(ref hostname) = h.hostname {
                    s.push_str(&format!(" ({hostname})"));
                }
                if let Some(ref user) = h.user {
                    s.push_str(&format!(" @{user}"));
                }
                s
            }
            PaletteItem::Recent(e) => {
                let prefix = match e {
                    tako_core::recent::RecentEntry::Directory { .. } => "dir",
                    tako_core::recent::RecentEntry::Repository { .. } => "repo",
                    tako_core::recent::RecentEntry::Ssh { .. } => "ssh",
                };
                format!("[{prefix}] {}", e.label())
            }
        }
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

impl Render for TakoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Issue #168: フレーム構築（element tree 生成）のメインスレッド専有を計測
        let _span = tako_control::diag::perf_span("render");
        self.drain_preview_image_evictions(window, cx);
        self.preview_device_scale = window.scale_factor();
        // Web ビュー（FR-3.8）: wry の親にするウィンドウハンドルは render でしか
        // 採取できないため、初回 render で保存し、復元待ちの Web ビューを開き直す
        if self.window_raw_handle.is_none() {
            self.window_raw_handle = webview::WindowHandleBox::from_window(window);
            if self.window_raw_handle.is_some() {
                self.restore_webviews();
            }
        }
        let cell = self.measure_cell(window);
        {
            let pane_ids: Vec<PaneId> = self.pane_font_sizes.keys().copied().collect();
            for pid in pane_ids {
                self.measure_pane_cell(pid, window);
            }
        }
        let theme = self.theme.clone();

        // ファイルツリーの root 同期は 2 秒ポーリングとイベント駆動に移した
        // （render 毎フレームの呼び出しを廃止してパフォーマンス改善）

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
            px(self.sidebar_width)
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

        // Web ビューの可視性同期: 今フレームで描画されなかったもの（非アクティブタブ・
        // dock 退避中）と、D&D 中の全 Web ビューを隠す（ネイティブビューは GPUI の
        // ドロップターゲット描画より上に来るため、ドラッグ中は GPUI 描画を優先する）
        let hide_webviews = self.drag_kind.is_some() && cx.has_active_drag();
        self.sync_webview_visibility(hide_webviews);

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

        let context_menu_overlay = self.render_context_menu(window, cx);
        let pane_context_overlay = self.render_pane_context_menu(window, cx);
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
                this.sync_filetree_roots();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PrevTab, _, cx| {
                this.workspace.activate_prev_tab();
                this.sync_filetree_roots();
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
            .on_action(cx.listener(|this, _: &SavePreview, _, cx| this.save_focused_preview(cx)))
            .on_action(cx.listener(|this, _: &UndoPreview, _, cx| {
                let pane_id = this.focused_pane();
                let _ = this.preview_undo_local(pane_id);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RedoPreview, _, cx| {
                let pane_id = this.focused_pane();
                let _ = this.preview_redo_local(pane_id);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FindPreview, _, cx| {
                let pane_id = this.focused_pane();
                if let Some(edit) = this.preview_edits.get_mut(&pane_id) {
                    edit.search_visible = !edit.search_visible;
                } else if this.previews.contains_key(&pane_id) {
                    if let Ok(mut new_edit) =
                        preview::EditState::open(this.previews.get(&pane_id).unwrap())
                    {
                        new_edit.editing = false;
                        new_edit.search_visible = true;
                        this.preview_edits.insert(pane_id, new_edit);
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.toggle_filetree();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                this.open_command_palette(window, cx);
            }))
            // Quit はここ（フォーカスパス依存）ではなく main() のグローバル
            // on_action + on_app_quit で処理する（#103）
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
            .on_action(cx.listener(|this, _: &OpenDirectory, _, cx| {
                this.open_directory(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenRepository, _, cx| {
                this.open_repository(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenRemote, _, cx| {
                this.open_ssh_palette(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenRecent, _, cx| {
                this.open_recent_palette(cx);
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.handle_key(&event.keystroke, cx);
            }))
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui::ModifiersChangedEvent, window, cx| {
                    this.on_modifiers_changed(event, window, cx);
                },
            ))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.on_mouse_up(event, cx);
                }),
            )
            .child(self.render_tab_bar(window, cx))
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
            .children(self.render_webview_dock(cx))
            .child(self.render_status_bar(cx))
            .child(ime_registration)
            .children(context_menu_overlay)
            .children(pane_context_overlay)
            .children(hover_preview_overlay)
            .children(pinned_overlays)
            .children(self.render_close_confirm_dialog(cx))
            .children(self.render_attention_toasts(cx))
            .children(self.render_command_palette(cx))
    }
}

impl TakoApp {
    /// 確認ダイアログ（Issue #172）。ウィンドウ全面を半透明背景で覆い、中央にダイアログを配置する
    fn render_close_confirm_dialog(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let target = self.pending_close_confirm?;
        let summary = self.close_summary(target);
        let theme = &self.theme;

        let label = match target {
            CloseConfirmTarget::Pane(_) => "ペインを閉じる",
            CloseConfirmTarget::Tab(_) => "タブを閉じる",
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000088))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.close_confirm_cancelled(cx);
                    }),
                )
                .child(
                    div()
                        .w(px(360.0))
                        .p_4()
                        .rounded(px(12.0))
                        .bg(rgba(theme.tab_bar_background))
                        .border_1()
                        .border_color(hsla(theme.border_subtle))
                        .shadow(vec![BoxShadow {
                            color: gpui::rgba(0x00000066).into(),
                            offset: point(px(0.), px(4.)),
                            blur_radius: px(24.0),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                        )
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hsla(theme.foreground))
                                .child(SharedString::from(label.to_string())),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(SharedString::from(summary)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .justify_end()
                                .child(
                                    div()
                                        .id("confirm-close-cancel")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(6.0))
                                        .cursor_pointer()
                                        .bg(rgba(theme.surface_highlight))
                                        .text_color(hsla(theme.foreground))
                                        .text_size(px(12.5))
                                        .hover(|d| d.bg(rgba_alpha(theme.surface_highlight, 1.5)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_confirm_cancelled(cx);
                                        }))
                                        .child("キャンセル (Esc)"),
                                )
                                .child(
                                    div()
                                        .id("confirm-close-ok")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(6.0))
                                        .cursor_pointer()
                                        .bg(rgba_alpha(theme.red, 0.3))
                                        .text_color(hsla(theme.red))
                                        .text_size(px(12.5))
                                        .hover(|d| d.bg(rgba_alpha(theme.red, 0.5)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_confirm_accepted(cx);
                                        }))
                                        .child("閉じる (Enter)"),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.text_overlay))
                                .child("⌘クリックで確認なしで閉じる"),
                        ),
                ),
        )
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
/// サイドバー用の軽量 git サマリ取得（#217。status + shortstat の 2 コマンドのみ）
fn fetch_sidebar_git(cwd: &std::path::Path) -> Option<SidebarGitSummary> {
    let repo = tako_core::git::repo_root(cwd)?;
    let status = tako_core::git::status(&repo);
    let (added_lines, removed_lines) = tako_core::git::diff_shortstat(&repo);
    Some(SidebarGitSummary {
        branch: status.branch,
        modified: status.entries.len(),
        added_lines,
        removed_lines,
    })
}

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

fn parse_initial_dir() -> Option<std::path::PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                if let Some(dir) = args.get(i + 1) {
                    let path = std::path::PathBuf::from(dir);
                    let path = if path.is_absolute() {
                        path
                    } else {
                        std::env::current_dir().unwrap_or_default().join(&path)
                    };
                    if path.is_dir() {
                        return Some(path);
                    } else {
                        eprintln!("warning: --dir の指定先が存在しない: {}", path.display());
                    }
                }
                i += 2;
            }
            arg if arg.starts_with("--dir=") => {
                let dir = &arg["--dir=".len()..];
                let path = std::path::PathBuf::from(dir);
                let path = if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir().unwrap_or_default().join(&path)
                };
                if path.is_dir() {
                    return Some(path);
                } else {
                    eprintln!("warning: --dir の指定先が存在しない: {}", path.display());
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// アプリメニューバーを構成する（File / Edit / Window）
fn app_menus() -> Vec<gpui::Menu> {
    use gpui::{Menu, MenuItem};
    vec![
        Menu::new("tako").items(vec![
            MenuItem::action("About tako", gpui::NoAction),
            MenuItem::separator(),
            MenuItem::action("Quit tako", Quit),
        ]),
        Menu::new("File").items(vec![
            MenuItem::action("New Window", NewWindow),
            MenuItem::separator(),
            MenuItem::action("Open Directory…", OpenDirectory),
            MenuItem::action("Open Repository…", OpenRepository),
            MenuItem::action("Open Remote…", OpenRemote),
            MenuItem::separator(),
            MenuItem::action("Open Recent…", OpenRecent),
            MenuItem::separator(),
            MenuItem::action("Save Preview", SavePreview),
        ]),
        Menu::new("Edit").items(vec![
            MenuItem::action("Copy", CopySelection),
            MenuItem::action("Paste", PasteClipboard),
            MenuItem::action("Select All", SelectAll),
        ]),
        Menu::new("View").items(vec![
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::separator(),
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Reset Zoom", ResetZoom),
        ]),
        Menu::new("Window").items(vec![
            MenuItem::action("New Tab", NewTab),
            MenuItem::separator(),
            MenuItem::action("Next Tab", NextTab),
            MenuItem::action("Previous Tab", PrevTab),
        ]),
    ]
}

/// カンプ準拠のタイトルバー統合設定（#217。タブバー行に traffic lights を同居させる）
fn tako_titlebar_options() -> gpui::TitlebarOptions {
    gpui::TitlebarOptions {
        title: Some("tako".into()),
        // システムタイトルバーを透過し、タブバー（44px）を最上段に置く
        appears_transparent: true,
        // 12px のライトをタブバー縦中央へ（(44-12)/2 = 16。カンプ padding-left 16）
        traffic_light_position: Some(point(px(16.), px(16.))),
    }
}

fn open_new_window(cx: &mut App) {
    open_window_with_bounds(
        WindowBounds::Windowed(Bounds::centered(None, size(px(960.), px(600.)), cx)),
        cx,
    );
}

/// 保存済みレイアウトから復元してウインドウを開く（#312: Dock クリックでの復帰用）
fn open_restored_window(cx: &mut App) {
    let saved_frame = tako_control::layout::load().and_then(|l| l.window);
    let bounds = match saved_frame {
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
    open_window_with_bounds(bounds, cx);
}

fn open_window_with_bounds(window_bounds: WindowBounds, cx: &mut App) {
    let _ = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(window_bounds),
                titlebar: Some(tako_titlebar_options()),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(TakoApp::new);
                window.focus(&view.read(cx).focus_handle.clone(), cx);
                // 赤ボタン close 時にレイアウトを保存する（#312。quit ではなく
                // ウインドウ単体の close なので on_app_quit は走らない）
                let entity = view.clone();
                window.on_window_should_close(cx, move |_window, cx| {
                    entity.update(cx, |app, _cx| app.save_layout());
                    true
                });
                view
            },
        )
        .expect("新規ウィンドウを開けなかった");
}

fn main() {
    // Issue #168: メインスレッド・ストール診断。重い区間（dispatch / render /
    // save_layout 等）の 2 秒超え継続を drop を待たず perf.log に記録する
    tako_control::diag::spawn_stall_watchdog();
    // 一括隔離モード（#177）: TAKO_ISOLATED=1 だけで本番リソース（layout.json /
    // tmux バックエンド / discovery）に一切触れない起動になる。実験・検証で個別の
    // 隔離変数を指定し漏らす事故（TAKO_DISCOVERY_DIR だけ隔離した dev 起動が
    // プライマリ判定 → 本番 layout を復元 → 稼働中インスタンスの tmux クライアントを
    // 強奪）への構造対策。個別変数が明示されていればそちらを尊重する
    if matches!(
        std::env::var("TAKO_ISOLATED").ok().as_deref(),
        Some("1" | "true" | "on")
    ) {
        if std::env::var_os("TAKO_PERSIST").is_none() {
            std::env::set_var("TAKO_PERSIST", "0");
        }
        if std::env::var_os("TAKO_TMUX_SOCKET").is_none() {
            std::env::set_var(
                "TAKO_TMUX_SOCKET",
                format!("tako-iso-{}", std::process::id()),
            );
        }
        if std::env::var_os("TAKO_DISCOVERY_DIR").is_none() {
            std::env::set_var(
                "TAKO_DISCOVERY_DIR",
                std::env::temp_dir().join(format!("tako-iso-discovery-{}", std::process::id())),
            );
        }
        // データディレクトリ（layout.json / settings.json / token / persist.log）も
        // 一括隔離する（#177 の穴: TAKO_ISOLATED + TAKO_PERSIST=1 の組み合わせで
        // 本番 layout.json を復元・上書きし得た）
        if std::env::var_os("TAKO_DATA_DIR").is_none() {
            std::env::set_var(
                "TAKO_DATA_DIR",
                std::env::temp_dir().join(format!("tako-iso-data-{}", std::process::id())),
            );
        }
        // セッションカタログ / ペインログ（Issue #112）も本番ファイルから隔離する
        if std::env::var_os("TAKO_SESSIONS_FILE").is_none() {
            std::env::set_var(
                "TAKO_SESSIONS_FILE",
                std::env::temp_dir().join(format!("tako-iso-sessions-{}.yaml", std::process::id())),
            );
        }
        if std::env::var_os("TAKO_PANE_LOG_DIR").is_none() {
            std::env::set_var(
                "TAKO_PANE_LOG_DIR",
                std::env::temp_dir().join(format!("tako-iso-pane-logs-{}", std::process::id())),
            );
        }
    }
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
    // セルフテストはセッションカタログ / ペインログ（Issue #112）も本番から隔離する
    if std::env::var_os("TAKO_SELF_TEST").is_some() {
        if std::env::var_os("TAKO_SESSIONS_FILE").is_none() {
            std::env::set_var(
                "TAKO_SESSIONS_FILE",
                std::env::temp_dir().join(format!("tako-st-sessions-{}.yaml", std::process::id())),
            );
        }
        if std::env::var_os("TAKO_PANE_LOG_DIR").is_none() {
            std::env::set_var(
                "TAKO_PANE_LOG_DIR",
                std::env::temp_dir().join(format!("tako-st-pane-logs-{}", std::process::id())),
            );
        }
    }
    // テレメトリ初期化: settings.json から ON/OFF を読み、panic ハンドラを設置する
    {
        let settings = tako_control::settings::load();
        tako_control::telemetry::init(settings.telemetry);
        tako_control::telemetry::install_panic_handler();
    }
    let initial_dir = parse_initial_dir();
    if let Some(ref dir) = initial_dir {
        if let Err(e) = std::env::set_current_dir(dir) {
            eprintln!("warning: --dir で指定されたディレクトリに移動できない: {e}");
        }
    }
    let app = application().with_assets(file_icons::TakoAssets);
    // Dock クリックでウインドウ再表示（#312。赤ボタン close 後にプロセスが
    // 生存している状態で Dock アイコンをクリックすると呼ばれる。ウインドウが
    // 無ければ保存済みレイアウトから復元して新規ウインドウを開く）
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_restored_window(cx);
            cx.activate(true);
        }
    });
    app.run(|cx: &mut App| {
        cx.bind_keys(key_bindings());
        cx.set_menus(app_menus());
        cx.on_action(|_: &NewWindow, cx| {
            open_new_window(cx);
        });
        // Quit はグローバルアクションとして登録する（#103: ルート div の on_action
        // だけだとウィンドウのフォーカスパス上でしか発火せず、フォーカス喪失
        // （blur）状態では cmd-q もメニューの Quit も無反応になる。GPUI の
        // アクションディスパッチは path 上で未処理ならグローバルハンドラの
        // Bubble フェーズへ必ず到達するため、ここならフォーカス状態・
        // ウィンドウ有無に依存しない）。layout 保存などの終了処理は
        // on_app_quit（TakoApp::new で登録。Dock 終了でも走る）が担う
        cx.on_action(|_: &Quit, cx| cx.quit());
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
                    titlebar: Some(tako_titlebar_options()),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(TakoApp::new);
                    window.focus(&view.read(cx).focus_handle.clone(), cx);
                    // 赤ボタン close 時にレイアウトを保存（#312）
                    let entity = view.clone();
                    window.on_window_should_close(cx, move |_window, cx| {
                        entity.update(cx, |app, _cx| app.save_layout());
                        true
                    });
                    view
                },
            )
            .expect("ウィンドウを開けなかった");
        cx.activate(true);

        if std::env::var_os("TAKO_VISUAL_TEST").is_some() {
            #[cfg(feature = "visual-test")]
            {
                self_test::run_visual(window, cx);
                return;
            }
            #[cfg(not(feature = "visual-test"))]
            {
                eprintln!("TAKO_VISUAL_TEST には --features visual-test が必要");
                std::process::exit(1);
            }
        }
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

    /// CoreGraphics / PDFKit の両方が受理する xref 付き最小 PDF を生成する。
    fn write_test_pdf(path: &std::path::Path) {
        write_test_pdf_with_text(path, "Hello PDF");
    }

    fn write_test_pdf_with_text(path: &std::path::Path, first_line: &str) {
        let content =
            format!("BT /F1 14 Tf 14 TL 72 700 Td ({first_line}) Tj T* (World 123) Tj ET");
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");
        let off1 = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let off2 = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        let off3 = pdf.len();
        pdf.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n");
        let off4 = pdf.len();
        pdf.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        pdf.extend_from_slice(content.as_bytes());
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        let off5 = pdf.len();
        pdf.extend_from_slice(
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
        );
        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in [off1, off2, off3, off4, off5] {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        std::fs::write(path, pdf).expect("テスト PDF を書ける");
    }

    /// PDFKit が `PDFDocument.outlineRoot` として読める 2 ページ・2 項目の PDF を生成する。
    fn write_test_pdf_with_outline(path: &std::path::Path) {
        let streams = [
            "BT /F1 14 Tf 14 TL 72 700 Td (Outline page one) Tj T* (World 123) Tj ET",
            "BT /F1 14 Tf 14 TL 72 700 Td (Outline page two) Tj T* (World 123) Tj ET",
        ];
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R /PageMode /UseOutlines >>"
                .to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                streams[0].len(),
                streams[0]
            )
            .into_bytes(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 7 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
            format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                streams[1].len(),
                streams[1]
            )
            .into_bytes(),
            b"<< /Type /Outlines /First 9 0 R /Last 10 0 R /Count 2 >>".to_vec(),
            b"<< /Title (Chapter One) /Parent 8 0 R /Next 10 0 R /Dest [3 0 R /Fit] >>"
                .to_vec(),
            b"<< /Title (Chapter Two) /Parent 8 0 R /Prev 9 0 R /Dest [6 0 R /Fit] >>"
                .to_vec(),
        ];

        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            pdf.extend_from_slice(object);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        std::fs::write(path, pdf).expect("アウトライン付きテスト PDF を書ける");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pdfkitアウトライン付きfixtureを読み込める() {
        let path = std::env::temp_dir().join(format!(
            "tako-preview-outline-test-{}.pdf",
            std::process::id()
        ));
        write_test_pdf_with_outline(&path);
        let state = preview::load_pdf(&path, 0);
        assert_eq!(state.outline.items.len(), 2);
        assert_eq!(state.outline.items[0].title, "Chapter One");
        assert_eq!(
            state.outline.items[1].target,
            tako_core::PreviewOutlineTarget::PdfPage { page: 2 }
        );
        let preview::PreviewContent::Pdf(data) = &state.content else {
            panic!("PDF fixture を読み込める");
        };
        assert!(data
            .text_layers
            .first()
            .is_some_and(|lines| lines.len() >= 2));
        let _ = std::fs::remove_file(path);
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

    /// 実 `tako` CLI をバックグラウンドで完了まで待つ。
    ///
    /// PTY へ文字入力して固定時間待つ方式では、シェルのコマンド開始・終了と app 状態の観測が
    /// 同期せず、処理が正しくても selftest 40 が偽失敗していた。CLI の IPC 応答は dispatch
    /// 完了後に返るため、子プロセス終了を状態検証の同期点にする。
    async fn cli_output_bg(
        cx: &AsyncApp,
        cli: &std::path::Path,
        endpoint: &str,
        token: &str,
        pane: PaneId,
        tab: TabId,
        args: Vec<String>,
    ) -> Option<std::process::Output> {
        let cli = cli.to_path_buf();
        let endpoint = endpoint.to_string();
        let token = token.to_string();
        let pane = pane.to_string();
        let tab = tab.to_string();
        cx.background_executor()
            .spawn(async move {
                std::process::Command::new(cli)
                    .args(args)
                    .env("TAKO_SOCKET", endpoint)
                    .env("TAKO_TOKEN", token)
                    .env("TAKO_PANE_ID", pane)
                    .env("TAKO_TAB_ID", tab)
                    .output()
                    .ok()
            })
            .await
    }

    /// OS イベント→デバウンス→background 読み込み→UI 差し替えの
    /// E2E 待ち合わせ。経過時間を受け入れ条件成立時の実測値として返す。
    async fn wait_for_preview_state<F>(
        window: WindowHandle<TakoApp>,
        cx: &mut AsyncApp,
        timeout: Duration,
        predicate: F,
    ) -> Option<Duration>
    where
        F: Fn(&TakoApp) -> bool,
    {
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
            if window
                .update(cx, |app, _, _| predicate(app))
                .unwrap_or(false)
            {
                return Some(started.elapsed());
            }
        }
        None
    }

    /// preview の次の paint で座標キャッシュが揃うまで待つ。
    /// `set_preview` が旧キャッシュを破棄するため、存在確認は新しい内容の描画完了を表す。
    async fn wait_for_preview_maps(
        any: AnyWindowHandle,
        window: WindowHandle<TakoApp>,
        cx: &mut AsyncApp,
        pane: PaneId,
        pdf: bool,
    ) -> bool {
        for _ in 0..40 {
            // typed WindowHandle<TakoApp>::update の内側で draw すると TakoApp の二重借用に
            // なる。root entity を借用しない AnyWindowHandle 境界から描画する。
            let _ = any.update(cx, |_, preview_window, cx| preview_window.draw(cx).clear());
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let ready = window
                .update(cx, |app, _, _| {
                    let has_text = app
                        .preview_line_texts
                        .get(&pane)
                        .is_some_and(|texts| !texts.is_empty());
                    if pdf {
                        has_text
                            && app
                                .preview_line_bounds
                                .get(&pane)
                                .is_some_and(|bounds| !bounds.is_empty())
                            && app
                                .preview_pdf_char_bounds
                                .get(&pane)
                                .is_some_and(|bounds| !bounds.is_empty())
                    } else {
                        has_text
                            && app
                                .preview_text_layouts
                                .get(&pane)
                                .is_some_and(|layouts| layouts.iter().any(Option::is_some))
                    }
                })
                .unwrap_or(false);
            if ready {
                return true;
            }
        }
        false
    }

    /// PDF 選択の最前面 canvas が実際に paint_quad を発行するまで、draw 完了を条件に待つ。
    async fn wait_for_pdf_highlight_paint(
        any: AnyWindowHandle,
        window: WindowHandle<TakoApp>,
        cx: &mut AsyncApp,
        pane: PaneId,
    ) -> bool {
        for _ in 0..40 {
            let _ = any.update(cx, |_, preview_window, cx| preview_window.draw(cx).clear());
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            if window
                .update(cx, |app, _, _| {
                    app.preview_pdf_highlight_paint_count
                        .get(&pane)
                        .copied()
                        .unwrap_or(0)
                        > 0
                })
                .unwrap_or(false)
            {
                return true;
            }
        }
        false
    }

    /// background syntect の完了と、その色付き内容を使った TextLayout の paint を待つ。
    async fn wait_for_preview_highlight(
        any: AnyWindowHandle,
        window: WindowHandle<TakoApp>,
        cx: &mut AsyncApp,
        pane: PaneId,
    ) -> bool {
        for _ in 0..80 {
            let _ = any.update(cx, |_, preview_window, cx| preview_window.draw(cx).clear());
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let ready = window
                .update(cx, |app, _, _| {
                    let color_count = match app.previews.get(&pane).map(|state| &state.content) {
                        Some(preview::PreviewContent::Code(lines)) => lines
                            .iter()
                            .flat_map(|line| line.iter())
                            .filter_map(|span| span.color)
                            .map(|color| (color.r, color.g, color.b))
                            .collect::<std::collections::HashSet<_>>()
                            .len(),
                        _ => 0,
                    };
                    color_count > 1
                        && app
                            .preview_text_layouts
                            .get(&pane)
                            .is_some_and(|layouts| layouts.iter().any(Option::is_some))
                })
                .unwrap_or(false);
            if ready {
                return true;
            }
        }
        false
    }

    /// Metal の最終 scene を直接 RGBA へ読み戻す。画面収録権限やウィンドウ前面化に
    /// 依存しない実ピクセル検証で、`--features visual-test` のときだけ有効。
    #[cfg(feature = "visual-test")]
    fn capture_frame(any: AnyWindowHandle, cx: &mut AsyncApp) -> Option<(image::RgbaImage, f32)> {
        any.update(cx, |_, window, cx| {
            window.draw(cx).clear();
            let scale = window.scale_factor();
            match window.render_to_image() {
                Ok(image) => Some((image, scale)),
                Err(error) => {
                    eprintln!("TAKO_VISUAL_CAPTURE_ERROR: {error:#}");
                    None
                }
            }
        })
        .ok()
        .flatten()
    }

    /// 指定した論理座標矩形内で RGBA が変化したピクセル数。Metal 読み戻しの上下方向が
    /// プラットフォームで異なる可能性を考慮し、通常 / Y 反転の多い方を採用する。
    #[cfg(feature = "visual-test")]
    fn changed_pixels_in_bounds(
        before: &image::RgbaImage,
        after: &image::RgbaImage,
        bounds: &[Bounds<Pixels>],
        scale: f32,
    ) -> usize {
        if before.dimensions() != after.dimensions() {
            return 0;
        }
        let (width, height) = before.dimensions();
        let count = |flip_y: bool| {
            let mut pixels = std::collections::HashSet::new();
            for bounds in bounds {
                let left = (f32::from(bounds.left()) * scale).floor().max(0.0) as u32;
                let right = (f32::from(bounds.right()) * scale)
                    .ceil()
                    .max(0.0)
                    .min(width as f32) as u32;
                let raw_top = (f32::from(bounds.top()) * scale).floor().max(0.0) as u32;
                let raw_bottom = (f32::from(bounds.bottom()) * scale)
                    .ceil()
                    .max(0.0)
                    .min(height as f32) as u32;
                let (top, bottom) = if flip_y {
                    (
                        height.saturating_sub(raw_bottom),
                        height.saturating_sub(raw_top),
                    )
                } else {
                    (raw_top.min(height), raw_bottom.min(height))
                };
                for y in top..bottom {
                    for x in left..right {
                        if before.get_pixel(x, y) != after.get_pixel(x, y) {
                            pixels.insert((x, y));
                        }
                    }
                }
            }
            pixels.len()
        };
        count(false).max(count(true))
    }

    /// サブラインスクロール（#159）の実ピクセル検証: `after` を `dy_logical`（論理 px）
    /// ぶん y 方向へ戻して `before` と比較する。ピクセル単位スクロールが機能していれば
    /// 「そのまま比較 = 大差分 / 戻して比較 = ほぼ一致」になる。
    /// 戻り値は (そのまま比較の差分, 戻して比較の差分)。Metal 読み戻しの上下方向差は
    /// changed_pixels_in_bounds と同様に両方向を試し、戻して比較が小さい方を採用する
    #[cfg(feature = "visual-test")]
    fn subline_shift_diff(
        before: &image::RgbaImage,
        after: &image::RgbaImage,
        bounds: &Bounds<Pixels>,
        scale: f32,
        dy_logical: f32,
    ) -> (usize, usize) {
        if before.dimensions() != after.dimensions() {
            return (0, usize::MAX);
        }
        let (width, height) = before.dimensions();
        let dy = (dy_logical * scale).round() as i64;
        let diff_pair = |flip_y: bool| -> (usize, usize) {
            let left = (f32::from(bounds.left()) * scale).floor().max(0.0) as u32;
            let right = (f32::from(bounds.right()) * scale)
                .ceil()
                .max(0.0)
                .min(width as f32) as u32;
            let raw_top = (f32::from(bounds.top()) * scale).floor().max(0.0) as u32;
            let raw_bottom = (f32::from(bounds.bottom()) * scale)
                .ceil()
                .max(0.0)
                .min(height as f32) as u32;
            let (top, bottom) = if flip_y {
                (
                    height.saturating_sub(raw_bottom),
                    height.saturating_sub(raw_top),
                )
            } else {
                (raw_top.min(height), raw_bottom.min(height))
            };
            // flip 時は論理 y とピクセル y が逆向きなのでシフトも反転する
            let shift = if flip_y { -dy } else { dy };
            let mut direct = 0usize;
            let mut shifted = 0usize;
            for y in top..bottom {
                let sy = y as i64 + shift;
                if sy < 0 || sy >= height as i64 {
                    continue;
                }
                for x in left..right {
                    if before.get_pixel(x, y) != after.get_pixel(x, y) {
                        direct += 1;
                    }
                    if before.get_pixel(x, y) != after.get_pixel(x, sy as u32) {
                        shifted += 1;
                    }
                }
            }
            (direct, shifted)
        };
        let a = diff_pair(false);
        let b = diff_pair(true);
        if a.1 <= b.1 {
            a
        } else {
            b
        }
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

    /// #152 専用の実描画ピクセル検証。通常セルフテストから独立させ、既存の PTY / fd
    /// ストレス項目のタイミングに左右されず PDF・C++・Python の scene だけを検査する。
    #[cfg(feature = "visual-test")]
    pub fn run_visual(window: WindowHandle<TakoApp>, cx: &mut App) {
        cx.spawn(async move |cx| {
            let any: AnyWindowHandle = window.into();
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let dir = std::env::temp_dir()
                .join(format!("tako-visual-preview-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("visual-test 一時ディレクトリを作れる");
            let pdf_path = dir.join("sample.pdf");
            write_test_pdf(&pdf_path);
            std::fs::write(
                dir.join("sample.cpp"),
                "#include <iostream>\nint main() { std::cout << \"hello\"; return 0; }\n",
            )
            .unwrap();
            std::fs::write(
                dir.join("sample.py"),
                "def greet(name):\n    message = f\"Hello {name}\"\n    return message\n",
            )
            .unwrap();

            let (base, pdf_pane) = window
                .update(cx, |app, _, cx| {
                    let base = app.focused_pane().as_u64();
                    let opened = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::OpenFile {
                            pane: Some(base),
                            path: pdf_path.display().to_string(),
                            mode: Some(tako_control::protocol::PreviewModeWire::Pdf),
                            direction: None,
                            focus: None,
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("visual-test PDF を dispatch で開ける");
                    cx.notify();
                    (
                        base,
                        PaneId::from_raw(
                            opened["pane"]
                                .as_u64()
                                .expect("OpenFile 応答には pane がある"),
                        ),
                    )
                })
                .unwrap_or_else(|_| fail("visual-test PDF dispatch"));
            check(
                wait_for_preview_maps(any, window, cx, pdf_pane, true).await,
                "visual-test PDF 文字矩形の paint",
            );
            let pdf_before = capture_frame(any, cx);
            window
                .update(cx, |app, _, cx| {
                    app.preview_selections.insert(
                        pdf_pane,
                        PreviewSelection {
                            anchor: (0, 0),
                            head: (0, "Hello".len()),
                        },
                    );
                    cx.notify();
                })
                .ok();
            check(
                wait_for_pdf_highlight_paint(any, window, cx, pdf_pane).await,
                "visual-test PDF 最前面 paint_quad",
            );
            let pdf_bounds: Vec<Bounds<Pixels>> = window
                .update(cx, |app, _, _| {
                    app.preview_pdf_char_bounds
                        .get(&pdf_pane)
                        .and_then(|lines| lines.first())
                        .map(|bounds| {
                            bounds
                                .iter()
                                .take("Hello".len())
                                .copied()
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let pdf_after = capture_frame(any, cx);
            let pdf_changed = match (pdf_before.as_ref(), pdf_after.as_ref()) {
                (Some((before, scale)), Some((after, _))) => {
                    changed_pixels_in_bounds(before, after, &pdf_bounds, *scale)
                }
                _ => 0,
            };
            println!("TAKO_VISUAL_PIXEL: pdf_selection changed={pdf_changed}");
            check(
                pdf_changed >= 8,
                "visual-test PDF 選択領域の RGBA ピクセル変化",
            );

            for (language, file_name) in [("cpp", "sample.cpp"), ("python", "sample.py")] {
                let opened = window
                    .update(cx, |app, _, cx| {
                        let opened = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::OpenFile {
                                pane: Some(pdf_pane.as_u64()),
                                path: dir.join(file_name).display().to_string(),
                                mode: Some(tako_control::protocol::PreviewModeWire::Code),
                                direction: None,
                                focus: None,
                            },
                            PaneOrigin::Cli,
                        )
                        .is_ok();
                        cx.notify();
                        opened
                    })
                    .unwrap_or(false);
                check(opened, &format!("visual-test {language} dispatch"));
                check(
                    wait_for_preview_maps(any, window, cx, pdf_pane, false).await,
                    &format!("visual-test {language} 平文 paint"),
                );
                let plain_frame = capture_frame(any, cx);
                window
                    .update(cx, |app, _, cx| app.drain_pending_highlights(cx))
                    .ok();
                check(
                    wait_for_preview_highlight(any, window, cx, pdf_pane).await,
                    &format!("visual-test {language} 読み取り色 paint"),
                );
                let read_frame = capture_frame(any, cx);
                let edit_colors = window
                    .update(cx, |app, _, cx| {
                        app.set_preview_editing_local(pdf_pane, true)
                            .expect("visual-test 編集開始");
                        cx.notify();
                        match app.previews.get(&pdf_pane).map(|state| &state.content) {
                            Some(preview::PreviewContent::Code(lines)) => lines
                                .iter()
                                .flat_map(|line| line.iter())
                                .filter_map(|span| span.color)
                                .map(|color| (color.r, color.g, color.b))
                                .collect::<std::collections::HashSet<_>>()
                                .len(),
                            _ => 0,
                        }
                    })
                    .unwrap_or(0);
                check(
                    edit_colors > 1,
                    &format!("visual-test {language} 編集構文色"),
                );
                let edit_frame = capture_frame(any, cx);
                let text_bounds = window
                    .update(cx, |app, _, _| {
                        app.preview_text_layouts
                            .get(&pdf_pane)
                            .map(|layouts| {
                                layouts
                                    .iter()
                                    .filter_map(|layout| layout.as_ref().map(TextLayout::bounds))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let read_changed = match (plain_frame.as_ref(), read_frame.as_ref()) {
                    (Some((plain, scale)), Some((read, _))) => {
                        changed_pixels_in_bounds(plain, read, &text_bounds, *scale)
                    }
                    _ => 0,
                };
                let edit_changed = match (plain_frame.as_ref(), edit_frame.as_ref()) {
                    (Some((plain, scale)), Some((edit, _))) => {
                        changed_pixels_in_bounds(plain, edit, &text_bounds, *scale)
                    }
                    _ => 0,
                };
                println!(
                    "TAKO_VISUAL_PIXEL: {language} read_changed={read_changed} edit_changed={edit_changed}"
                );
                check(
                    read_changed >= 8,
                    &format!("visual-test {language} 読み取り RGBA"),
                );
                check(
                    edit_changed >= 8,
                    &format!("visual-test {language} 編集 RGBA"),
                );
                window
                    .update(cx, |app, _, _| {
                        app.set_preview_editing_local(pdf_pane, false)
                            .expect("visual-test 編集終了");
                    })
                    .ok();
            }

            window
                .update(cx, |app, _, _| {
                    let _ = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Close {
                            pane: Some(pdf_pane.as_u64()),
                            force: true,
                        },
                        PaneOrigin::Cli,
                    );
                    let _ = app.workspace.active_tab_mut().tree_mut().focus(PaneId::from_raw(base));
                })
                .ok();
            let _ = std::fs::remove_dir_all(&dir);

            // ターミナルのサブラインスクロール（#159）: 半行スクロールの前後フレームを
            // 「そのまま」と「半行戻して」比較する。ピクセル単位で描画されていれば
            // 戻して比較のみほぼ一致する（行単位描画だと 1 行ずれとなりどちらも大差分）
            type_text(any, cx, "seq 100", true);
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            let (term_area, cell_h) = window
                .update(cx, |app, _, cx| {
                    let pane = app.focused_pane();
                    if let Some(s) = app.terminals.get(&pane) {
                        s.scroll_to_bottom();
                    }
                    cx.notify();
                    let area = app
                        .pane_text_areas
                        .iter()
                        .find(|(id, _)| *id == pane)
                        .map(|(_, b)| *b);
                    let ch = app
                        .cell_size_for_pane(pane)
                        .map(|c| f32::from(c.height))
                        .unwrap_or(17.0);
                    (area, ch)
                })
                .unwrap_or((None, 17.0));
            let term_area = term_area.unwrap_or_else(|| fail("visual-test ターミナル領域"));
            let scroll_before = capture_frame(any, cx);
            window
                .update(cx, |app, win, cx| {
                    let pane = app.focused_pane();
                    app.on_pane_scroll(
                        pane,
                        &ScrollWheelEvent {
                            position: term_area.center(),
                            delta: ScrollDelta::Pixels(point(px(0.0), px(cell_h * 0.5))),
                            ..ScrollWheelEvent::default()
                        },
                        win,
                        cx,
                    );
                    cx.notify();
                })
                .ok();
            let scroll_after = capture_frame(any, cx);
            // 上下 2 行（部分行・カーソル行）と右端 16px（スクロールバー）を除いた内側で比較
            let inset = Bounds::new(
                point(term_area.origin.x, term_area.origin.y + px(cell_h * 2.0)),
                size(
                    term_area.size.width - px(16.0),
                    term_area.size.height - px(cell_h * 4.0),
                ),
            );
            let (direct, shifted) = match (scroll_before.as_ref(), scroll_after.as_ref()) {
                (Some((b, scale)), Some((a, _))) => {
                    subline_shift_diff(b, a, &inset, *scale, cell_h * 0.5)
                }
                _ => (0, usize::MAX),
            };
            println!("TAKO_VISUAL_PIXEL: subline direct={direct} shifted={shifted}");
            check(direct > 200, "visual-test サブライン: 半行スクロールで画面が動く");
            check(
                shifted.saturating_mul(4) < direct,
                "visual-test サブライン: 半行戻しでほぼ一致（ピクセル単位描画）",
            );
            window
                .update(cx, |app, _, cx| {
                    let pane = app.focused_pane();
                    if let Some(s) = app.terminals.get(&pane) {
                        s.scroll_to_bottom();
                    }
                    cx.notify();
                })
                .ok();

            if let Some(discovery) = std::env::var_os("TAKO_DISCOVERY_DIR") {
                let _ = std::fs::remove_dir_all(discovery);
            }
            println!("TAKO_VISUAL_TEST_OK");
            std::process::exit(0);
        })
        .detach();
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

            // 17. tako list がペイン内シェルから成功する（FR-2.2.4 / FR-2.2.7）。
            //     高負荷環境では debug ビルドの CLI 起動 + IPC 往復が 1 秒を超えることが
            //     あるため、リトライループで待つ（フレーキー対策）
            type_text(
                any,
                cx,
                &format!("{cli} list >/dev/null && echo TAKO-LIST-$((40+2))"),
                true,
            );
            let mut list_ok = false;
            for _ in 0..8 {
                wait(cx, 800).await;
                list_ok = focused_contains(window, cx, "TAKO-LIST-42");
                if list_ok {
                    break;
                }
            }
            check(list_ok, "tako list");

            // 18. tako split --down --focus（呼び出し元の自動特定 + origin=cli + フォーカス移動）。
            // 既定はフォーカスを分割元に維持する仕様（3c9d363）のため、--focus を明示して
            // 新ペインへ移す（後続テストは新ペインからの入力を前提とする）
            type_text(any, cx, &format!("{cli} split --down --focus"), true);
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
            check(pane4 != pane2, "split --focus 後フォーカスは新ペイン");
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

            // 25. tako tab new（FR-2.5.10。AI 操作の既定はフォーカス維持のため、
            //     後続操作で新タブを使うこのテストは --focus を明示する）
            type_text(
                any,
                cx,
                &format!("{cli} tab new --title agents --focus"),
                true,
            );
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
            check(status == 200 && tool_count == 99, "MCP tools/list は 99 ツール");

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

            // 33b. tools/call tako_theme（Issue #217。set light → GUI 反映 → toggle で復帰）
            let (status, response) = mcp_post_bg(
                cx,
                &mcp_url,
                Some(&token),
                &[],
                r#"{"jsonrpc":"2.0","id":103,"method":"tools/call","params":{"name":"tako_theme","arguments":{"action":"set","mode":"light"}}}"#,
            )
            .await
            .unwrap_or_else(|| fail("MCP theme set 接続"));
            let light_applied = window
                .update(cx, |app, _, _| {
                    app.theme.mode == tako_core::theme::ThemeMode::Light
                })
                .unwrap_or(false);
            check(
                status == 200 && response.contains(r#"\"theme\":\"light\""#) && light_applied,
                "MCP theme set light（GUI 反映）",
            );
            let (status, response) = mcp_post_bg(
                cx,
                &mcp_url,
                Some(&token),
                &[],
                r#"{"jsonrpc":"2.0","id":104,"method":"tools/call","params":{"name":"tako_theme","arguments":{"action":"toggle"}}}"#,
            )
            .await
            .unwrap_or_else(|| fail("MCP theme toggle 接続"));
            let dark_restored = window
                .update(cx, |app, _, _| {
                    app.theme.mode == tako_core::theme::ThemeMode::Dark
                })
                .unwrap_or(false);
            check(
                status == 200 && response.contains(r#"\"theme\":\"dark\""#) && dark_restored,
                "MCP theme toggle（dark へ復帰）",
            );

            // 33c. ライブリロードの MCP 切替は core 状態と 1:1（#233）。
            //      一度 OFF にした後 ON へ戻し、後続 E2E で実監視を使う。
            let (status, response) = mcp_post_bg(
                cx,
                &mcp_url,
                Some(&token),
                &[],
                r#"{"jsonrpc":"2.0","id":105,"method":"tools/call","params":{"name":"tako_preview_reload","arguments":{"enabled":false}}}"#,
            )
            .await
            .unwrap_or_else(|| fail("MCP preview reload off 接続"));
            let preview_reload_off = window
                .update(cx, |app, _, _| !app.preview_reload.enabled())
                .unwrap_or(false);
            check(
                status == 200
                    && response.contains(r#"\"enabled\":false"#)
                    && preview_reload_off,
                "MCP preview reload off（core 状態へ反映）",
            );
            let (status, response) = mcp_post_bg(
                cx,
                &mcp_url,
                Some(&token),
                &[],
                r#"{"jsonrpc":"2.0","id":106,"method":"tools/call","params":{"name":"tako_preview_reload","arguments":{"enabled":true}}}"#,
            )
            .await
            .unwrap_or_else(|| fail("MCP preview reload on 接続"));
            let preview_reload_on = window
                .update(cx, |app, _, _| app.preview_reload.enabled())
                .unwrap_or(false);
            check(
                status == 200
                    && response.contains(r#"\"enabled\":true"#)
                    && preview_reload_on,
                "MCP preview reload on（core 状態へ反映）",
            );

            // 33d. プレビュー画像キャッシュ上限も MCP から同じ LRU へ反映する（#258）。
            let (status, response) = mcp_post_bg(
                cx,
                &mcp_url,
                Some(&token),
                &[],
                r#"{"jsonrpc":"2.0","id":107,"method":"tools/call","params":{"name":"tako_preview_cache","arguments":{"max_mb":256}}}"#,
            )
            .await
            .unwrap_or_else(|| fail("MCP preview cache 接続"));
            let preview_cache_256 = window
                .update(cx, |app, _, _| app.preview_image_lru.budget_bytes() == 256 * 1024 * 1024)
                .unwrap_or(false);
            check(
                status == 200
                    && response.contains(r#"\"max_mb\":256"#)
                    && preview_cache_256,
                "MCP preview cache（LRU 予算へ反映）",
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
            //     （2026-06-11 常用報告: 根分割の崩しで panic→SIGABRT）。実 CLI の終了を
            //     同期点にし、固定待ち時間でコマンド完了を推測しない（#145 引き継ぎ時の偽失敗）。
            let (caller_pane, caller_tab) = window
                .update(cx, |app, _, _| {
                    (app.focused_pane(), app.workspace.active_tab_id())
                })
                .unwrap_or_else(|_| fail("回帰 40: 呼び出し元の状態取得"));
            let tab_new = cli_output_bg(
                cx,
                &cli_path,
                &ipc_endpoint,
                &token,
                caller_pane,
                caller_tab,
                vec![
                    "tab".into(),
                    "new".into(),
                    "--title".into(),
                    "close-reg".into(),
                    "--focus".into(),
                ],
            )
            .await
            .unwrap_or_else(|| fail("回帰 40: tab new CLI 起動"));
            check(tab_new.status.success(), "回帰 40: tab new CLI 応答");
            let reg_pane_a = window
                .update(cx, |app, _, _| app.workspace.active_tab().tree().focused())
                .unwrap_or_else(|_| fail("回帰 40: タブ作成後の状態取得"));
            // 既定はフォーカス維持の仕様（3c9d363）のため --focus で新ペインへ移す
            let reg_tab = window
                .update(cx, |app, _, _| app.workspace.active_tab_id())
                .unwrap_or_else(|_| fail("回帰 40: split 前のタブ取得"));
            let split = cli_output_bg(
                cx,
                &cli_path,
                &ipc_endpoint,
                &token,
                reg_pane_a,
                reg_tab,
                vec!["split".into(), "--right".into(), "--focus".into()],
            )
            .await
            .unwrap_or_else(|| fail("回帰 40: split CLI 起動"));
            check(split.status.success(), "回帰 40: split CLI 応答");
            let reg_pane_b = window
                .update(cx, |app, _, _| app.workspace.active_tab().tree().focused())
                .unwrap_or_else(|_| fail("回帰 40: split 後の状態取得"));
            check(reg_pane_b != reg_pane_a, "回帰 40: split で新ペイン");
            // 旧ペインへフォーカスを戻し、新ペイン（非フォーカス側）を外から閉じる
            let focus = cli_output_bg(
                cx,
                &cli_path,
                &ipc_endpoint,
                &token,
                reg_pane_b,
                reg_tab,
                vec!["focus".into(), reg_pane_a.to_string()],
            )
            .await
            .unwrap_or_else(|| fail("回帰 40: focus CLI 起動"));
            check(focus.status.success(), "回帰 40: focus CLI 応答");
            let close = cli_output_bg(
                cx,
                &cli_path,
                &ipc_endpoint,
                &token,
                reg_pane_a,
                reg_tab,
                vec!["close".into(), "--pane".into(), reg_pane_b.to_string()],
            )
            .await
            .unwrap_or_else(|| fail("回帰 40: close CLI 起動"));
            check(close.status.success(), "回帰 40: close CLI 応答");
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
            // close 後の PTY / プロセス破棄は非同期のため、高負荷環境では回収が
            // 遅延する。fd 数が落ち着くまでリトライして待つ（真のリークなら待っても
            // 減らないので検査の意味は変わらない）
            let mut fd_ok = false;
            let mut fd_after = 0;
            for _ in 0..10 {
                fd_after = std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0);
                if fd_after <= fd_before + 8 {
                    fd_ok = true;
                    break;
                }
                wait(cx, 800).await;
            }
            check(
                fd_ok,
                &format!("split/close で fd が漏れない（before={fd_before}, after={fd_after}）"),
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

            // 41b. split が分割元の cwd を継承する（OSC 7 連携。FR-2.4.1）。
            //     --focus で新ペインへ移り、新ペイン側の cwd 継承を検証する（3c9d363 追従）
            type_text(any, cx, &format!("{cli} split --right --focus"), true);
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

            // 44b. ピクセル単位スムーススクロール（#159）: トラックパッドの Pixels デルタが
            //      display_offset（整数）+ fract（サブライン端数）へ分解され、部分行描画用の
            //      extra_bottom が付く。端数計算はユニットテスト側（subline_scroll）で網羅
            let wheel_pixels = |app: &mut TakoApp,
                                win: &mut Window,
                                cx: &mut Context<TakoApp>,
                                dy: f32| {
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
                        delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
                        ..ScrollWheelEvent::default()
                    },
                    win,
                    cx,
                );
            };
            let (half_offset, half_fract, half_extra, half_shift_px) = window
                .update(cx, |app, win, cx| {
                    let pane = app.focused_pane();
                    if let Some(s) = app.terminals.get(&pane) {
                        s.scroll_to_bottom();
                    }
                    let cell_h = app
                        .cell_size_for_pane(pane)
                        .map(|c| f32::from(c.height))
                        .unwrap_or(17.0);
                    // 半行ぶん上（過去）へ = 正のピクセルデルタ
                    wheel_pixels(app, win, cx, cell_h * 0.5);
                    let session = app.terminals.get(&pane).expect("セッションはある");
                    let screen = session.screen(&app.theme);
                    (
                        session.display_offset(),
                        session.scroll_subline_fract(),
                        screen.extra_bottom.is_some(),
                        session.scroll_subline_fract() * cell_h,
                    )
                })
                .unwrap_or((0, 0.0, false, 0.0));
            check(
                half_offset == 1 && (half_fract - 0.5).abs() < 0.01,
                "Pixels 半行で display_offset 1 + fract 0.5 に分解",
            );
            check(half_extra, "サブライン中は extra_bottom（部分行）が付く");
            check(half_shift_px > 1.0, "描画シフト量が正のピクセル値");
            let (back_offset, back_fract, typed_reset) = window
                .update(cx, |app, win, cx| {
                    let pane = app.focused_pane();
                    let cell_h = app
                        .cell_size_for_pane(pane)
                        .map(|c| f32::from(c.height))
                        .unwrap_or(17.0);
                    // 半行戻す → 最下部（offset 0 / fract 0）
                    wheel_pixels(app, win, cx, -cell_h * 0.5);
                    let session = app.terminals.get(&pane).expect("セッションはある");
                    let back = (session.display_offset(), session.scroll_subline_fract());
                    // 再び半行遡ってからキー入力 → write() が最下部へ戻し fract もリセット
                    wheel_pixels(app, win, cx, cell_h * 0.5);
                    if let Some(s) = app.terminals.get(&pane) {
                        s.write(b" ".to_vec());
                    }
                    let session = app.terminals.get(&pane).expect("セッションはある");
                    let reset = session.display_offset() == 0
                        && session.scroll_subline_fract() == 0.0;
                    (back.0, back.1, reset)
                })
                .unwrap_or((99, 9.0, false));
            check(
                back_offset == 0 && back_fract == 0.0,
                "半行戻しで最下部へスナップ",
            );
            check(typed_reset, "キー入力でサブライン位置がリセット");

            // 任意のピクセル検証停止点（通常の self-test では待機しない）。
            // fract=0 と fract=0.5 の 2 状態で停止し、外部の screencapture が
            // サブライン描画（同一内容が半行ずれる）を実ピクセルで比較できるようにする
            if std::env::var_os("TAKO_SELF_TEST_SCROLL_VISUAL").is_some() {
                let _ = window.update(cx, |app, _, cx| {
                    let pane = app.focused_pane();
                    if let Some(s) = app.terminals.get(&pane) {
                        s.scroll_to_bottom();
                    }
                    cx.notify();
                });
                println!("TAKO_SCROLL_VISUAL_BASELINE_READY");
                wait(cx, 15_000).await;
                let _ = window.update(cx, |app, win, cx| {
                    let pane = app.focused_pane();
                    let cell_h = app
                        .cell_size_for_pane(pane)
                        .map(|c| f32::from(c.height))
                        .unwrap_or(17.0);
                    wheel_pixels(app, win, cx, cell_h * 0.5);
                    cx.notify();
                });
                println!("TAKO_SCROLL_VISUAL_HALF_READY");
                wait(cx, 15_000).await;
                let _ = window.update(cx, |app, _, cx| {
                    let pane = app.focused_pane();
                    if let Some(s) = app.terminals.get(&pane) {
                        s.scroll_to_bottom();
                    }
                    cx.notify();
                });
            }

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

            // 45b. Shift+Enter が GUI キー経路で CSI u（\e[13;2u）としてペインへ届く
            //     （Issue #28 回帰防止: kitty 未要求でも ModifiedOnly が全ペイン既定。
            //     tmux バックエンドは extended-keys always が csi-u のまま内側へ届け、
            //     直接 spawn はそのまま届く。cat 実行中の cooked TTY は ECHOCTL で
            //     ESC を ^[ とエコーするため、届いたバイト列が ^[[13;2u として見える）
            type_text(any, cx, "cat", true);
            wait(cx, 800).await;
            press(any, cx, "shift-enter");
            let mut shift_enter_csi_u = false;
            for _ in 0..20 {
                wait(cx, 200).await;
                if focused_contains(window, cx, "[13;2u") {
                    shift_enter_csi_u = true;
                    break;
                }
            }
            check(
                shift_enter_csi_u,
                "Shift+Enter が CSI u で届く（Issue #28）",
            );
            press(any, cx, "ctrl-c");
            wait(cx, 500).await;

            // 45c.（任意・TAKO_SELF_TEST_CLAUDE=1 のときだけ）実 claude で Shift+Enter が
            //     改行として入力欄に入る e2e（Issue #28 の受け入れ検証そのもの。
            //     セルフテストは tmux バックエンド OFF = 直接 spawn なので、
            //     まさに Issue #28 で死んでいた経路を実 claude で通す。
            //     claude CLI + 認証が必要なため既定ではスキップ。verify-claude-mcp.sh と同格の
            //     実機検証ツールという位置付け）
            if std::env::var_os("TAKO_SELF_TEST_CLAUDE").is_some() {
                type_text(any, cx, "claude", true);
                let mut claude_ready = false;
                for _ in 0..80 {
                    wait(cx, 500).await;
                    if focused_contains(window, cx, "trust this folder")
                        || focused_contains(window, cx, "Yes, I trust")
                    {
                        press(any, cx, "enter");
                        continue;
                    }
                    if focused_contains(window, cx, "shift+tab to cycle") {
                        claude_ready = true;
                        break;
                    }
                }
                check(claude_ready, "45c: claude TUI 起動");
                wait(cx, 2000).await;
                type_text(any, cx, "hello28", false);
                wait(cx, 1000).await;
                press(any, cx, "shift-enter");
                wait(cx, 1000).await;
                type_text(any, cx, "world28", false);
                // 入力欄内で hello28 の直下の行に world28 が来る = 改行挿入
                // （送信されてしまった場合は transcript とセパレータを挟むため隣接しない。
                //  改行されなかった場合は同一行に連結される）
                let mut newline_ok = false;
                for _ in 0..20 {
                    wait(cx, 400).await;
                    let ok = window
                        .update(cx, |app, _, _| {
                            app.focused_session()
                                .map(|s| {
                                    let lines = s.visible_lines();
                                    let joined = lines
                                        .iter()
                                        .any(|l| l.contains("hello28") && l.contains("world28"));
                                    let hello =
                                        lines.iter().position(|l| l.contains("hello28"));
                                    let world =
                                        lines.iter().position(|l| l.contains("world28"));
                                    matches!((hello, world), (Some(h), Some(w)) if w == h + 1)
                                        && !joined
                                })
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if ok {
                        newline_ok = true;
                        break;
                    }
                }
                // 判定根拠の画面証跡を出力（成否に関わらず。ペインの spawn 経路も明示）
                let _ = window.update(cx, |app, _, _| {
                    let pane = app.focused_pane();
                    println!(
                        "45c-PANE: pane={pane} backend={}",
                        app.backend_sessions.contains_key(&pane)
                    );
                    if let Some(s) = app.focused_session() {
                        for l in s.visible_lines() {
                            if !l.trim().is_empty() {
                                println!("45c-SCREEN: {l}");
                            }
                        }
                    }
                });
                check(newline_ok, "45c: 実claudeでShift+Enterが改行（#28）");
                // 片付け: 入力破棄 → claude 終了 → シェル復帰を確認。
                // claude は「非空入力の ctrl-c = クリア」「空入力の ctrl-c = 終了確認 →
                // もう一度で終了」のため 3 連打（終了後の余分な ctrl-c はシェルに無害）
                press(any, cx, "ctrl-c");
                wait(cx, 400).await;
                press(any, cx, "ctrl-c");
                wait(cx, 400).await;
                press(any, cx, "ctrl-c");
                wait(cx, 2500).await;
                type_text(any, cx, "echo BACK28", true);
                let mut back_to_shell = false;
                for _ in 0..20 {
                    wait(cx, 400).await;
                    if focused_contains(window, cx, "BACK28") {
                        back_to_shell = true;
                        break;
                    }
                }
                check(back_to_shell, "45c: claude 終了後にシェルへ復帰");
            }

            // 46. 全角行のマウス座標→セル変換。描画は 1 文字 = 1 div（w = cell_width × char_cols）
            //     でグリッドスナップするため、全角文字「う」の描画位置（cell_width × 4）を
            //     クリックしたとき、グリッド col = 4（全角 2 セル × 2 文字ぶん）に解決されること
            press(any, cx, "ctrl-u");
            type_text(any, cx, "echo あいうえおかきくけこ", true);
            // 高負荷環境では echo の実行・画面反映が 1 秒を超えることがあるため
            // リトライで待つ（座標解決の実バグならリトライ後も false のまま fail する）
            let mut wide_hit = false;
            for _ in 0..8 {
                wait(cx, 800).await;
                wide_hit = window
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
                        // 描画は cell_width 基準（1 文字 = 1 div でグリッドスナップ）なので
                        // クリック位置も cell_width × グリッド列で求める
                        let target_col = line.cell_cols[2]; // "う" のグリッド列
                        let x = f32::from(cell.width) * target_col as f32 + 2.0;
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
                if wide_hit {
                    break;
                }
            }
            check(wide_hit, "全角行のクリックが正しいセルに解決");

            // 47. ペインの × ボタン = kill（dispatch 共有経路）。split で増やして × 相当の
            //     操作でアクティブタブから片付き、ターミナル（プロセス）も破棄され、バックグラウンドにも
            //     残らないこと。タブの × と挙動を統一し、紐づく tmux セッションも
            //     remove_pane が kill するため管理外 / orphan に残らない。
            //     --focus で新ペインを対象にする（3c9d363 追従。分割元の誤 kill 防止）
            type_text(
                any,
                cx,
                &format!("{cli} split --right --focus >/dev/null"),
                true,
            );
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
            //      （ターミナル）は生かしたまま、ツリーから外れて shelved へ移ること（FR-2.15.1）。
            //      --focus で新ペインを対象にする（3c9d363 追従。分割元の誤退避防止）
            type_text(
                any,
                cx,
                &format!("{cli} split --right --focus >/dev/null"),
                true,
            );
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
                            sidebar_width: None,
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
                            sidebar_width: None,
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
                            sidebar_width: None,
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
            //     dispatch 経路。セルフテスト中は設定・レイアウトを永続化しない）。
            //     診断フィールド（layout_path / last_restore。Issue #30）の露出も確認する
            type_text(
                any,
                cx,
                &format!(
                    "{cli} persist on >/dev/null && {cli} persist \
                     | grep -q '\"enabled\":true' && {cli} persist \
                     | grep -q '\"last_restore\"' && echo TAKO-PS-$((50+8))"
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

                // 61b. バックエンドのホイール = tmux 履歴のローカルミラー表示（#159）。
                //      copy-mode には入らず、capture した履歴 + ライブ画面の合成行列で
                //      過去が見える（ピクセル単位スクロールの土台）
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
                            let mirrored = app
                                .scroll_ctls
                                .get(&backend_pane)
                                .is_some_and(|c| c.mirror_scrolling());
                            // 合成行列がライブ viewport と異なる = 過去が見えている
                            let composed_differs = app
                                .terminals
                                .get(&backend_pane)
                                .map(|s| s.screen(&app.theme))
                                .and_then(|screen| {
                                    let composed =
                                        app.compose_mirror_lines(backend_pane, &screen)?;
                                    Some(composed.first()?.text != screen.lines.first()?.text)
                                })
                                .unwrap_or(false);
                            // copy-mode には入っていない（キー飲まれの構造的解消）
                            let no_copy_mode = tako_core::scroll::scroll_state(
                                &tako_core::scroll::ScrollTarget::Backend {
                                    socket: backend_sock.clone(),
                                    session: backend_name.clone(),
                                },
                            )
                            .map(|s| !s.in_mode)
                            .unwrap_or(false);
                            mirrored && composed_differs && no_copy_mode
                        })
                        .unwrap_or(false);
                    if wheel_scrolled {
                        break;
                    }
                }
                check(
                    wheel_scrolled,
                    "バックエンドのホイールがミラー表示に乗る（copy-mode 不使用）",
                );

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

                // 61d. スクロール中のキー入力は最下部（ライブ表示）へ戻してから流れる
                //      （iTerm2 流。ミラー方式では copy-mode に入らないため
                //      「キーが飲まれる」事故は構造的に起きない）
                press(any, cx, "enter");
                let mut key_cancelled = false;
                for _ in 0..20 {
                    wait(cx, 300).await;
                    key_cancelled = window
                        .update(cx, |app, _, _| {
                            app.scroll_ctls
                                .get(&backend_pane)
                                .map(|c| !c.mirror_scrolling())
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

                // 61e. CLI（dispatch 共有）でもバックエンドのミラー表示位置に効く
                //      （開発不変条件: UI と同じ層を CLI / MCP からも操作できる）
                press(any, cx, "ctrl-u");
                type_text(any, cx, &format!("{cli} scroll --to 5 >/dev/null"), true);
                let mut cli_scrolled = false;
                for _ in 0..20 {
                    wait(cx, 300).await;
                    cli_scrolled = window
                        .update(cx, |app, _, _| {
                            app.scroll_ctls
                                .get(&backend_pane)
                                .map(|c| {
                                    let pos = c
                                        .mirror
                                        .as_ref()
                                        .map(|m| m.position)
                                        .unwrap_or(c.pending_rows);
                                    pos >= 4.5
                                })
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if cli_scrolled {
                        break;
                    }
                }
                check(cli_scrolled, "tako scroll がバックエンドのミラー表示位置に効く");

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
                &format!("{cli} split --down --focus -- sh -c 'echo TAKO-CMD-\"OK\"; sleep 15'"),
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
            let mut code_fixture = String::from(
                "fn mixed() {\n\tlet label = \"日本語 abc\";\n}\n",
            );
            for line in 3..90 {
                code_fixture.push_str(&format!("let value_{line} = {line};\n"));
            }
            std::fs::write(preview_dir.join("hello.rs"), code_fixture).unwrap();
            std::fs::write(
                preview_dir.join("note.md"),
                "# Title\n\n日本語 abc\tend\n\n- item\n",
            )
            .unwrap();
            let mut outline_markdown = String::from("# Overview\n\n");
            for index in 0..24 {
                outline_markdown.push_str(&format!("paragraph before {index}\n\n"));
            }
            outline_markdown.push_str("## Duplicate\n\n");
            for index in 0..24 {
                outline_markdown.push_str(&format!("paragraph middle {index}\n\n"));
            }
            outline_markdown.push_str("## Duplicate\n\n");
            for index in 0..40 {
                outline_markdown.push_str(&format!("paragraph after {index}\n\n"));
            }
            let outline_md_path = preview_dir.join("outline.md");
            std::fs::write(&outline_md_path, outline_markdown).unwrap();
            let no_heading_md_path = preview_dir.join("no-heading.md");
            std::fs::write(&no_heading_md_path, "本文だけです。\n\n- 項目\n").unwrap();
            std::fs::write(
                preview_dir.join("sample.cpp"),
                "#include <iostream>\nint main() { std::cout << \"hello\"; return 0; }\n",
            )
            .unwrap();
            std::fs::write(
                preview_dir.join("sample.py"),
                "def greet(name):\n    message = f\"Hello {name}\"\n    return message\n",
            )
            .unwrap();
            let pdf_path = preview_dir.join("sample.pdf");
            write_test_pdf(&pdf_path);
            let outlined_pdf_path = preview_dir.join("outlined.pdf");
            write_test_pdf_with_outline(&outlined_pdf_path);
            let plain_pdf_path = preview_dir.join("plain.pdf");
            write_test_pdf(&plain_pdf_path);
            let image_path = preview_dir.join("sample.png");
            image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
                .save(&image_path)
                .expect("テスト PNG を書ける");
            let selftest_open =
                |app: &mut TakoApp, base: u64, path: String, mode| {
                    tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::OpenFile {
                            pane: Some(base),
                            path,
                            mode,
                            direction: None,
                            focus: None,
                        },
                        PaneOrigin::Cli,
                    )
                };
            let (base, pane, code_ok, md_open_ok) = window
                .update(cx, |app, _, cx| {
                    let base = app.focused_pane().as_u64();
                    // コードを開く: ペインが生え、PTY は起動しない。フォーカスは移る
                    let opened = selftest_open(
                        app,
                        base,
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
                    let opened = selftest_open(
                        app,
                        base,
                        preview_dir.join("note.md").display().to_string(),
                        None,
                    )
                    .expect("md の open_file は成功する");
                    app.drain_pending_preview_loads(cx);
                    let md_open_ok = opened["pane"].as_u64() == Some(pane)
                        && opened["created"].as_bool() == Some(false)
                        && opened["mode"].as_str() == Some("markdown")
                        && matches!(
                            app.previews.values().next().map(|p| &p.content),
                            Some(preview::PreviewContent::Loading)
                        );
                    (base, pane, code_ok, md_open_ok)
                })
                .unwrap_or((0, 0, false, false));
            let mut md_ok = false;
            for _ in 0..30 {
                wait(cx, 100).await;
                md_ok = md_open_ok
                    && window
                        .update(cx, |app, _, _| {
                            app.previews.values().next().is_some_and(|state| {
                                matches!(
                                    &state.content,
                                    preview::PreviewContent::Markdown(blocks)
                                        if matches!(blocks.first(),
                                            Some(preview::MdBlock::Heading { level: 1, .. }))
                                ) && state.outline.items.len() == 1
                                    && state.outline.items[0].title == "Title"
                            })
                        })
                        .unwrap_or(false);
                if md_ok {
                    break;
                }
            }
            let outline_md_opened = window
                .update(cx, |app, _, cx| {
                    let opened = selftest_open(
                        app,
                        base,
                        outline_md_path.display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Markdown),
                    )
                    .is_ok();
                    app.drain_pending_preview_loads(cx);
                    opened
                })
                .unwrap_or(false);
            let outline_md_loaded = wait_for_preview_state(
                window,
                cx,
                Duration::from_secs(3),
                |app| {
                    app.previews
                        .values()
                        .next()
                        .is_some_and(|state| state.outline.items.len() == 3)
                },
            )
            .await
            .is_some();
            // 新しい本文の子矩形を確定させ、ジャンプ前は 3 項目目が画面外であることを測る。
            let _ = any.update(cx, |_, preview_window, cx| preview_window.draw(cx).clear());
            wait(cx, 50).await;
            let markdown_target_position = |app: &TakoApp| -> Option<(f32, f32, f32)> {
                let target = app
                    .previews
                    .values()
                    .next()
                    .and_then(|state| state.outline.target(3).ok())?;
                let tako_core::PreviewOutlineTarget::MarkdownBlock { block } = target else {
                    return None;
                };
                let handle = app.preview_scroll_handles.get(&PaneId::from_raw(pane))?;
                let target_bounds = handle.bounds_for_item(block)?;
                Some((
                    f32::from(target_bounds.top() + handle.offset().y),
                    f32::from(handle.bounds().top()),
                    f32::from(handle.bounds().bottom()),
                ))
            };
            let markdown_before = window
                .update(cx, |app, _, _| markdown_target_position(app))
                .ok()
                .flatten();
            let markdown_outline_ok = window
                .update(cx, |app, _, _| {
                    let listed = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::PreviewOutline {
                            pane: Some(pane),
                            item: None,
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("Markdown アウトラインを一覧できる");
                    outline_md_opened
                        && outline_md_loaded
                        && listed["outline"].as_array().is_some_and(|items| {
                            items.len() == 3
                                && items[1]["title"] == "Duplicate"
                                && items[2]["title"] == "Duplicate"
                                && items[1]["target"] != items[2]["target"]
                        })
                })
                .unwrap_or(false);
            let markdown_jump_requested = window
                .update(cx, |app, _, cx| {
                    let jumped = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::PreviewOutline {
                            pane: Some(pane),
                            item: Some(3),
                        },
                        PaneOrigin::Mcp,
                    )
                    .expect("Markdown アウトラインの 3 項目目へジャンプできる");
                    cx.notify();
                    jumped["selected"]["kind"] == "markdown_block"
                })
                .unwrap_or(false);
            let _ = any.update(cx, |_, preview_window, cx| preview_window.draw(cx).clear());
            wait(cx, 50).await;
            let markdown_after = window
                .update(cx, |app, _, _| markdown_target_position(app))
                .ok()
                .flatten();
            let (markdown_jump_ok, markdown_jump_delta) =
                match (markdown_before, markdown_after) {
                    (Some((before_top, _, before_bottom)), Some((after_top, top, bottom))) => (
                        markdown_jump_requested
                            && before_top > before_bottom
                            && after_top >= top - 1.0
                            && after_top < bottom,
                        (after_top - top).abs(),
                    ),
                    _ => (false, f32::INFINITY),
                };
            let no_heading_opened = window
                .update(cx, |app, _, cx| {
                    let opened = selftest_open(
                        app,
                        base,
                        no_heading_md_path.display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Markdown),
                    )
                    .is_ok();
                    app.drain_pending_preview_loads(cx);
                    opened
                })
                .unwrap_or(false);
            let no_heading_ok = wait_for_preview_state(
                window,
                cx,
                Duration::from_secs(3),
                |app| {
                    app.previews.values().next().is_some_and(|state| {
                        matches!(state.content, preview::PreviewContent::Markdown(_))
                            && state.outline.is_empty()
                    })
                },
            )
            .await
            .is_some()
                && no_heading_opened;
            // PDF を開く: 応答は即返り（Loading プレースホルダ）、全ページラスタライズは
            // background（Issue #168）。実運用では IPC ループ / UI が drain するが、
            // ここは直接 dispatch のため手動で流し、完了をポーリングで待つ
            let pdf_open_ok = window
                .update(cx, |app, _, cx| {
                    let opened = selftest_open(
                        app,
                        base,
                        pdf_path.display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Pdf),
                    )
                    .expect("pdf の open_file は成功する");
                    app.drain_pending_preview_loads(cx);
                    opened["pane"].as_u64() == Some(pane)
                        && opened["created"].as_bool() == Some(false)
                        && opened["mode"].as_str() == Some("pdf")
                })
                .unwrap_or(false);
            let mut pdf_ok = false;
            for _ in 0..30 {
                wait(cx, 200).await;
                pdf_ok = pdf_open_ok
                    && window
                        .update(cx, |app, _, _| {
                            matches!(
                                app.previews.values().next().map(|p| &p.content),
                                Some(preview::PreviewContent::Pdf(data))
                                    if !data.pages.is_empty()
                                        && !data.text_layers.is_empty()
                                        && data.text_layers[0].len() >= 2
                                        && data.text_layers[0][0].char_boxes.len()
                                            == data.text_layers[0][0].text.chars().count()
                            )
                        })
                        .unwrap_or(false);
                if pdf_ok {
                    break;
                }
            }
            let outlined_pdf_opened = window
                .update(cx, |app, _, cx| {
                    let opened = selftest_open(
                        app,
                        base,
                        outlined_pdf_path.display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Pdf),
                    )
                    .is_ok();
                    app.drain_pending_preview_loads(cx);
                    opened
                })
                .unwrap_or(false);
            let outlined_pdf_loaded = wait_for_preview_state(
                window,
                cx,
                Duration::from_secs(5),
                |app| {
                    app.previews.values().next().is_some_and(|state| {
                        matches!(state.content, preview::PreviewContent::Pdf(_))
                            && state.outline.items.len() == 2
                    })
                },
            )
            .await
            .is_some();
            // background 完了後の PDF ページ矩形を確定してからジャンプ要求を出す。
            let _ = any.update(cx, |_, preview_window, cx| preview_window.draw(cx).clear());
            wait(cx, 50).await;
            let pdf_page_position = |app: &TakoApp| -> Option<(f32, f32, f32, f32, f32)> {
                let handle = app.preview_scroll_handles.get(&PaneId::from_raw(pane))?;
                let page_bounds = handle.bounds_for_item(1)?;
                Some((
                    f32::from(handle.offset().y),
                    f32::from(page_bounds.top() + handle.offset().y),
                    f32::from(page_bounds.bottom() + handle.offset().y),
                    f32::from(handle.bounds().top()),
                    f32::from(handle.bounds().bottom()),
                ))
            };
            let pdf_before = window
                .update(cx, |app, _, _| pdf_page_position(app))
                .ok()
                .flatten();
            let pdf_outline_ok = window
                .update(cx, |app, _, _| {
                    let listed = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::PreviewOutline {
                            pane: Some(pane),
                            item: None,
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("PDF アウトラインを一覧できる");
                    let jumped = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::PreviewOutline {
                            pane: Some(pane),
                            item: Some(2),
                        },
                        PaneOrigin::Mcp,
                    )
                    .expect("PDF アウトラインの 2 項目目へジャンプできる");
                    listed["outline"].as_array().is_some_and(|items| {
                        outlined_pdf_opened
                            && outlined_pdf_loaded
                            && items.len() == 2
                            && items[0]["title"] == "Chapter One"
                            && items[1]["title"] == "Chapter Two"
                    }) && jumped["selected"]["kind"] == "pdf_page"
                        && jumped["selected"]["page"] == 2
                        && app
                            .preview_views
                            .get(&PaneId::from_raw(pane))
                            .is_some_and(|view| view.page == 2)
                })
                .unwrap_or(false);
            let _ = any.update(cx, |_, preview_window, cx| preview_window.draw(cx).clear());
            wait(cx, 50).await;
            let pdf_after = window
                .update(cx, |app, _, _| pdf_page_position(app))
                .ok()
                .flatten();
            let (pdf_jump_ok, pdf_jump_delta) = match (pdf_before, pdf_after) {
                (
                    Some((before_offset, before_page_top, _, _, _)),
                    Some((after_offset, after_page_top, after_page_bottom, top, bottom)),
                ) => (
                    after_offset < before_offset - 1.0
                        && after_page_top < before_page_top - 1.0
                        && after_page_top < bottom
                        && after_page_bottom > top,
                    (after_page_top - top).abs(),
                ),
                _ => (false, f32::INFINITY),
            };
            let plain_pdf_opened = window
                .update(cx, |app, _, cx| {
                    let opened = selftest_open(
                        app,
                        base,
                        plain_pdf_path.display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Pdf),
                    )
                    .is_ok();
                    app.drain_pending_preview_loads(cx);
                    opened
                })
                .unwrap_or(false);
            let plain_pdf_outline_ok = wait_for_preview_state(
                window,
                cx,
                Duration::from_secs(5),
                |app| {
                    app.previews.values().next().is_some_and(|state| {
                        matches!(state.content, preview::PreviewContent::Pdf(_))
                            && state.outline.is_empty()
                    })
                },
            )
            .await
            .is_some()
                && plain_pdf_opened;
            let (mode_ok, toggle_ok, list_ok, closed) = window
                .update(cx, |app, _, cx| {
                    // dispatch の mode 指定（CLI --mode / MCP mode と同じ）でコード表示へ
                    let opened = selftest_open(
                        app,
                        base,
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
                        tako_control::protocol::Request::Close { pane: Some(pane), force: true },
                        PaneOrigin::Cli,
                    )
                    .is_ok()
                        && app.previews.is_empty();
                    (mode_ok, toggle_ok, list_ok, closed)
                })
                .unwrap_or((false, false, false, false));
            println!(
                "TAKO_PREVIEW_OUTLINE_DEBUG: md_list_ok={markdown_outline_ok} md_jump_ok={markdown_jump_ok} md_jump_delta_px={markdown_jump_delta:.1} pdf_list_ok={pdf_outline_ok} pdf_jump_ok={pdf_jump_ok} pdf_jump_delta_px={pdf_jump_delta:.1}"
            );
            check(code_ok, "コードプレビューの open");
            check(md_ok, "Markdown プレビューの再利用");
            check(markdown_outline_ok, "Markdown 重複見出しの一覧と個別ターゲット");
            check(markdown_jump_ok, "Markdown 目次ジャンプ後の見出し先頭位置");
            check(no_heading_ok, "見出しなし Markdown は空アウトラインへ劣化");
            check(pdf_ok, "PDF プレビューの文字矩形抽出（background 読み込み完了後）");
            check(
                pdf_outline_ok && pdf_jump_ok,
                "PDFKit アウトライン一覧と 2 ページ目ジャンプ",
            );
            check(plain_pdf_outline_ok, "目次なし PDF は空アウトラインへ劣化");
            check(mode_ok, "プレビューモード指定");
            check(toggle_ok, "プレビューモードの UI トグル");
            check(list_ok, "プレビュー状態の list 公開");
            check(closed, "プレビューペインの close");
            println!(
                "TAKO_PREVIEW_OUTLINE: markdown_items=3 duplicate_targets=distinct markdown_jump_delta_px={markdown_jump_delta:.1} no_heading=empty pdf_items=2 pdf_page=2 pdf_jump_delta_px={pdf_jump_delta:.1} no_pdf_outline=empty"
            );

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

            // 66b-2. プレビュー座標の機械検証（#145）。描画に使った GPUI TextLayout の
            // position_for_index → 実 preview_hit_test を往復し、行頭 / 行末 / 日本語 /
            // タブを確認する。旧 cell_width 換算値も証拠として出力する。
            let md_pane = window
                .update(cx, |app, preview_window, cx| {
                    let pane = app.previews.keys().next().copied()?;
                    if let Some(tab) = app
                        .workspace
                        .tabs()
                        .iter()
                        .find(|tab| tab.tree().contains(pane))
                        .map(|tab| tab.id())
                    {
                        let _ = app.workspace.activate_tab(tab);
                        app.scroll_active_tab_into_view();
                        let _ = app.workspace.active_tab_mut().tree_mut().focus(pane);
                    }
                    preview_window.refresh();
                    cx.notify();
                    Some(pane)
                })
                .ok()
                .flatten()
                .unwrap_or_else(|| fail("座標検証用 Markdown preview がある"));
            check(
                wait_for_preview_maps(any, window, cx, md_pane, false).await,
                "Markdown の座標キャッシュ生成",
            );
            let (md_coordinates_ok, md_coordinate_record) = window
                .update(cx, |app, _, _| {
                    let pane = *app.previews.keys().next().expect("preview がある");
                    let texts = app.preview_line_texts.get(&pane).expect("md texts がある");
                    let layouts = app
                        .preview_text_layouts
                        .get(&pane)
                        .expect("md layouts がある");
                    let hit = |line: usize, byte: usize| {
                        let layout = layouts[line].as_ref().expect("text layout がある");
                        let mut position = layout
                            .position_for_index(byte)
                            .expect("byte の描画位置がある");
                        position.y += layout.line_height() / 2.0;
                        app.preview_hit_test(pane, position)
                    };
                    let paragraph = texts
                        .iter()
                        .position(|text| text == "日本語 abc\tend")
                        .expect("日本語 paragraph がある");
                    let paragraph_text = &texts[paragraph];
                    let tab_end = paragraph_text.find('\t').expect("tab がある") + 1;
                    let heading_layout = layouts[0].as_ref().expect("heading layout がある");
                    let heading_position = heading_layout
                        .position_for_index(3)
                        .expect("heading の位置がある");
                    let old_cell = app.cell_size.map(|size| size.width).unwrap_or(px(8.0));
                    let old_col = ((heading_position.x - heading_layout.bounds().left()) / old_cell)
                        .floor() as usize;
                    let old_byte = texts[0]
                        .char_indices()
                        .nth(old_col)
                        .map(|(byte, _)| byte)
                        .unwrap_or(texts[0].len());
                    let expected = [
                        (0, 0),
                        (0, 3),
                        (0, texts[0].len()),
                        (paragraph, 0),
                        (paragraph, "日本語 ".len()),
                        (paragraph, tab_end),
                        (paragraph, paragraph_text.len()),
                    ];
                    let samples: Vec<_> = expected
                        .into_iter()
                        .map(|expected| (expected, hit(expected.0, expected.1)))
                        .collect();
                    (
                        samples
                            .iter()
                            .all(|(expected, actual)| Some(*expected) == *actual),
                        format!(
                            "md heading byte=3 x={:.2} old_cell_byte={} shaped_byte=3; 日本語tab byte={}; hits={samples:?}",
                            f32::from(heading_position.x - heading_layout.bounds().left()),
                            old_byte,
                            tab_end,
                        ),
                    )
                })
                .unwrap_or((false, String::new()));
            println!("TAKO_PREVIEW_COORD: {md_coordinate_record}");
            check(
                md_coordinates_ok,
                "Markdown の行頭 / 行末 / 日本語 / tab 座標が shaping と往復する",
            );

            // コード表示へ差し替え、同じ往復と overflow scroll 後のウィンドウ座標更新を確認。
            let code_pane = window
                .update(cx, |app, _, cx| {
                    let pane = app.focused_pane().as_u64();
                    let opened = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::OpenFile {
                            pane: Some(pane),
                            path: preview_dir.join("hello.rs").display().to_string(),
                            mode: Some(tako_control::protocol::PreviewModeWire::Code),
                            direction: None,
                            focus: None,
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("座標検証用 code を開ける");
                    cx.notify();
                    opened["pane"].as_u64().expect("pane が返る")
                })
                .unwrap_or(0);
            check(
                wait_for_preview_maps(any, window, cx, PaneId::from_raw(code_pane), false).await,
                "コードの座標キャッシュ生成",
            );
            let (code_coordinates_ok, code_coordinate_record, line0_before) = window
                .update(cx, |app, _, _| {
                    let pane = PaneId::from_raw(code_pane);
                    let texts = app.preview_line_texts.get(&pane).expect("code texts がある");
                    let layouts = app
                        .preview_text_layouts
                        .get(&pane)
                        .expect("code layouts がある");
                    let roundtrip = |line: usize, byte: usize| {
                        let layout = layouts[line].as_ref().expect("text layout がある");
                        let mut position = layout
                            .position_for_index(byte)
                            .expect("byte の描画位置がある");
                        position.y += layout.line_height() / 2.0;
                        app.preview_hit_test(pane, position) == Some((line, byte))
                    };
                    let mixed = &texts[1];
                    let japanese_end = mixed.find(" abc").expect("日本語がある");
                    let tab_end = mixed.find('\t').expect("tab がある") + 1;
                    let layout = layouts[1].as_ref().expect("mixed layout がある");
                    let position = layout
                        .position_for_index(japanese_end)
                        .expect("日本語末尾の位置がある");
                    (
                        roundtrip(0, 0)
                            && roundtrip(0, texts[0].len())
                            && roundtrip(1, japanese_end)
                            && roundtrip(1, tab_end)
                            && roundtrip(1, mixed.len()),
                        format!(
                            "code mixed byte={} x={:.2} tab_byte={}",
                            japanese_end,
                            f32::from(position.x - layout.bounds().left()),
                            tab_end,
                        ),
                        layouts[0]
                            .as_ref()
                            .expect("line 0 layout がある")
                            .bounds()
                            .top(),
                    )
                })
                .unwrap_or((false, String::new(), px(0.0)));
            println!("TAKO_PREVIEW_COORD: {code_coordinate_record}");
            check(
                code_coordinates_ok,
                "コードの行頭 / 行末 / 日本語 / tab 座標が shaping と往復する",
            );
            window
                .update(cx, |app, window, cx| {
                    let pane = PaneId::from_raw(code_pane);
                    let position = app
                        .preview_text_layouts
                        .get(&pane)
                        .and_then(|layouts| layouts.first())
                        .and_then(Option::as_ref)
                        .map(TextLayout::bounds)
                        .map(|bounds| bounds.center())
                        .unwrap_or_default();
                    window.dispatch_event(
                        gpui::PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Pixels(point(px(0.0), px(-500.0))),
                            ..ScrollWheelEvent::default()
                        }),
                        cx,
                    );
                    // 次の paint 完了を存在で同期できるよう、旧位置のキャッシュを破棄する。
                    app.preview_line_bounds.remove(&pane);
                    app.preview_pdf_char_bounds.remove(&pane);
                    app.preview_pdf_page_image_bounds.remove(&pane);
                    app.preview_text_layouts.remove(&pane);
                    app.preview_line_texts.remove(&pane);
                })
                .ok();
            check(
                wait_for_preview_maps(any, window, cx, PaneId::from_raw(code_pane), false).await,
                "スクロール後の座標キャッシュ更新",
            );
            let scroll_coordinates_ok = window
                .update(cx, |app, _, _| {
                    let pane = PaneId::from_raw(code_pane);
                    let texts = app.preview_line_texts.get(&pane).expect("code texts がある");
                    let layouts = app
                        .preview_text_layouts
                        .get(&pane)
                        .expect("code layouts がある");
                    let line = 40;
                    let layout = layouts[line].as_ref().expect("scroll line layout がある");
                    let byte = texts[line].len();
                    let mut position = layout
                        .position_for_index(byte)
                        .expect("scroll 後の描画位置がある");
                    position.y += layout.line_height() / 2.0;
                    println!(
                        "TAKO_PREVIEW_COORD: scroll line=40 before_y={:.2} after_line0_y={:.2} target_y={:.2} byte={}",
                        f32::from(line0_before),
                        f32::from(layouts[0].as_ref().expect("line 0").bounds().top()),
                        f32::from(position.y),
                        byte,
                    );
                    layouts[0].as_ref().expect("line 0").bounds().top() < line0_before
                        && app.preview_hit_test(pane, position) == Some((line, byte))
                })
                .unwrap_or(false);
            check(
                scroll_coordinates_ok,
                "スクロール後も code 座標が更新されて同じ文字位置へ往復する",
            );

            let pdf_opened = window
                .update(cx, |app, _, cx| {
                    let pane = PaneId::from_raw(code_pane);
                    let opened = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::OpenFile {
                            pane: Some(pane.as_u64()),
                            path: pdf_path.display().to_string(),
                            mode: Some(tako_control::protocol::PreviewModeWire::Pdf),
                            direction: None,
                            focus: None,
                        },
                        PaneOrigin::Cli,
                    )
                    .expect("pdf を開ける");
                    // PDF は background 読み込み（Issue #168）: 直接 dispatch のため手動 drain。
                    // 完了は直後の wait_for_preview_maps が待つ
                    app.drain_pending_preview_loads(cx);
                    cx.notify();
                    opened["mode"].as_str() == Some("pdf")
                })
                .unwrap_or(false);
            check(pdf_opened, "座標検証用 PDF を開く");
            check(
                wait_for_preview_maps(any, window, cx, PaneId::from_raw(code_pane), true).await,
                "PDF の座標キャッシュ生成",
            );
            let pdf_coordinates_ok = window
                .update(cx, |app, _, cx| {
                    let pane = PaneId::from_raw(code_pane);
                    let texts = app
                        .preview_line_texts
                        .get(&pane)
                        .expect("pdf texts がある");
                    let line_bounds = app
                        .preview_line_bounds
                        .get(&pane)
                        .expect("pdf line bounds がある");
                    let char_bounds = app
                        .preview_pdf_char_bounds
                        .get(&pane)
                        .expect("pdf char bounds がある");
                    let line0 = texts.first().expect("pdf line 0 がある");
                    let line0_chars = char_bounds.first().expect("pdf char line 0 がある");
                    let first = line0_chars.first().expect("pdf char 0 がある");
                    let last = line0_chars.last().expect("pdf 末尾 char がある");
                    let caret_point = |bounds: &Bounds<Pixels>, fraction: f32| {
                        point(
                            bounds.left() + (bounds.right() - bounds.left()) * fraction,
                            bounds.center().y,
                        )
                    };
                    let first_start = caret_point(first, 0.25);
                    let last_start = caret_point(last, 0.25);
                    let last_end = caret_point(last, 0.75);
                    let first_byte = 0usize;
                    let last_byte = line0
                        .char_indices()
                        .last()
                        .map(|(byte, _)| byte)
                        .unwrap_or(0);
                    let hit_start = app.preview_hit_test(pane, first_start);
                    let hit_last_start = app.preview_hit_test(pane, last_start);
                    let hit_end = app.preview_hit_test(pane, last_end);
                    app.preview_selections.insert(
                        pane,
                        PreviewSelection {
                            anchor: (0, 0),
                            head: (0, "Hello".len()),
                        },
                    );
                    cx.notify();
                    let copied = app.preview_selected_text().as_deref() == Some("Hello");
                    println!(
                        "TAKO_PREVIEW_COORD: pdf first_x={:.2} last_x={:.2} hits={hit_start:?}/{hit_last_start:?}/{hit_end:?}",
                        f32::from(first_start.x),
                        f32::from(last_end.x),
                    );
                    !line_bounds.is_empty()
                        && !char_bounds.is_empty()
                        && hit_start == Some((0, first_byte))
                        && hit_last_start == Some((0, last_byte))
                        && hit_end == Some((0, line0.len()))
                        && copied
                })
                .unwrap_or(false);
            check(
                pdf_coordinates_ok,
                "PDF の文字矩形ヒットテストと選択コピーが往復する",
            );
            let preview_zoom_command_ok = window
                .update(cx, |app, preview_window, cx| {
                    let pane = PaneId::from_raw(code_pane);
                    let result = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::PreviewView {
                            pane: Some(pane.as_u64()),
                            zoom: Some(150.0),
                            zoom_in: false,
                            zoom_out: false,
                            reset: false,
                            page: Some(1),
                            pan_x: None,
                            pan_y: None,
                        },
                        PaneOrigin::Mcp,
                    )
                    .expect("PDF を 150% へズームできる");
                    preview_window.refresh();
                    cx.notify();
                    result["zoom"].as_f64() == Some(150.0)
                        && result["page"].as_u64() == Some(1)
                })
                .unwrap_or(false);
            check(
                preview_zoom_command_ok,
                "PDF の page + zoom を dispatch で同時指定できる",
            );
            let mut zoom_coordinates_ok = false;
            let mut zoom_coordinate_record = String::new();
            for _ in 0..60 {
                // refresh/notify は次フレームを要求するだけなので、ラスタ完了後の canvas
                // 座標まで同期するには root entity を借用しない draw を明示する。
                let _ = any.update(cx, |_, preview_window, cx| {
                    preview_window.draw(cx).clear()
                });
                wait(cx, 100).await;
                let observation = window
                    .update(cx, |app, preview_window, cx| {
                        preview_window.refresh();
                        cx.notify();
                        let pane = PaneId::from_raw(code_pane);
                        let zoom = app.preview_views.get(&pane).map(|view| view.zoom);
                        let raster_zoom = app.previews.get(&pane).and_then(|state| {
                            if let preview::PreviewContent::Pdf(data) = &state.content {
                                Some(data.raster_key.zoom_percent)
                            } else {
                                None
                            }
                        });
                        let hit = app
                            .preview_pdf_char_bounds
                            .get(&pane)
                            .and_then(|lines| lines.first())
                            .and_then(|line| line.first())
                            .map(|bounds| {
                                app.preview_hit_test(
                                    pane,
                                    point(bounds.left() + px(1.0), bounds.center().y),
                                )
                            });
                        (zoom, raster_zoom, hit)
                    })
                    .unwrap_or((None, None, None));
                zoom_coordinates_ok = observation.0 == Some(1.5)
                    && observation.1 == Some(150)
                    && observation.2 == Some(Some((0, 0)));
                zoom_coordinate_record = format!(
                    "zoom={:?} raster_zoom={:?} hit={:?}",
                    observation.0, observation.1, observation.2
                );
                if zoom_coordinates_ok {
                    break;
                }
            }
            println!("TAKO_PREVIEW_COORD: pdf zoom {zoom_coordinate_record}");
            check(
                zoom_coordinates_ok,
                "PDF 150% の再ラスタライズ後も文字座標が画像へ追従する",
            );
            let pinch_position = window
                .update(cx, |app, _, _| {
                    app.preview_scroll_handles
                        .get(&PaneId::from_raw(code_pane))
                        .map(|handle| handle.bounds().center())
                })
                .ok()
                .flatten()
                .unwrap_or_else(|| fail("PDF pinch の中心座標がある"));
            let dispatch_pinch = |delta: f32, cx: &mut AsyncApp| {
                [
                    (gpui::TouchPhase::Started, 0.0),
                    (gpui::TouchPhase::Moved, delta),
                    (gpui::TouchPhase::Ended, 0.0),
                ]
                .into_iter()
                .all(|(phase, phase_delta)| {
                    any.update(cx, |_, preview_window, cx| {
                        preview_window.dispatch_event(
                            gpui::PlatformInput::Pinch(gpui::PinchEvent {
                                position: pinch_position,
                                delta: phase_delta,
                                phase,
                                ..gpui::PinchEvent::default()
                            }),
                            cx,
                        );
                    })
                    .is_ok()
                })
            };
            check(dispatch_pinch(0.1, cx), "PDF へ pinch-in イベントを送れる");
            wait(cx, 50).await;
            let pinch_in_zoom = window
                .update(cx, |app, _, _| {
                    app.preview_views
                        .get(&PaneId::from_raw(code_pane))
                        .map(|view| view.zoom)
                })
                .ok()
                .flatten()
                .unwrap_or_default();
            check(pinch_in_zoom > 1.5, "pinch-in で PDF 倍率が増える");
            check(dispatch_pinch(-0.1, cx), "PDF へ pinch-out イベントを送れる");
            wait(cx, 50).await;
            let pinch_out_zoom = window
                .update(cx, |app, _, _| {
                    app.preview_views
                        .get(&PaneId::from_raw(code_pane))
                        .map(|view| view.zoom)
                })
                .ok()
                .flatten()
                .unwrap_or_default();
            check(
                pinch_out_zoom < pinch_in_zoom,
                "pinch-out で PDF 倍率が減る",
            );
            println!(
                "TAKO_PREVIEW_COORD: pdf pinch in={pinch_in_zoom:.3} out={pinch_out_zoom:.3}"
            );
            check(
                wait_for_pdf_highlight_paint(
                    any,
                    window,
                    cx,
                    PaneId::from_raw(code_pane),
                )
                .await,
                "PDF 選択が最前面 canvas の paint_quad へ到達",
            );
            // C++ / Python は background syntect 完了後の読み取り状態と編集状態で
            // 複数色を確認する。実ピクセル比較は独立した TAKO_VISUAL_TEST で行う。
            for (language, file_name) in [("cpp", "sample.cpp"), ("python", "sample.py")] {
                let pane = PaneId::from_raw(code_pane);
                let opened = window
                    .update(cx, |app, _, cx| {
                        let opened = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::OpenFile {
                                pane: Some(code_pane),
                                path: preview_dir.join(file_name).display().to_string(),
                                mode: Some(tako_control::protocol::PreviewModeWire::Code),
                                direction: None,
                                focus: None,
                            },
                            PaneOrigin::Cli,
                        )
                        .is_ok();
                        cx.notify();
                        opened
                    })
                    .unwrap_or(false);
                check(opened, &format!("{language} の読み取りプレビューを開く"));
                check(
                    wait_for_preview_maps(any, window, cx, pane, false).await,
                    &format!("{language} 平文フレームの描画"),
                );
                window
                    .update(cx, |app, _, cx| app.drain_pending_highlights(cx))
                    .ok();
                check(
                    wait_for_preview_highlight(any, window, cx, pane).await,
                    &format!("{language} 読み取りの syntect 色付き描画"),
                );
                let editing_colors = window
                    .update(cx, |app, _, cx| {
                        app.set_preview_editing_local(pane, true)
                            .expect("コード編集を開始できる");
                        cx.notify();
                        match app.previews.get(&pane).map(|state| &state.content) {
                            Some(preview::PreviewContent::Code(lines)) => lines
                                .iter()
                                .flat_map(|line| line.iter())
                                .filter_map(|span| span.color)
                                .map(|color| (color.r, color.g, color.b))
                                .collect::<std::collections::HashSet<_>>()
                                .len(),
                            _ => 0,
                        }
                    })
                    .unwrap_or(0);
                check(
                    editing_colors > 1,
                    &format!("{language} 編集表示も複数の構文色を保持"),
                );
                window
                    .update(cx, |app, _, _| {
                        app.set_preview_editing_local(pane, false)
                            .expect("コード編集を終了できる");
                    })
                    .ok();
            }
            // 後続の編集 e2e は note.md を対象にするため Markdown へ戻す。
            window
                .update(cx, |app, _, cx| {
                    let _ = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::OpenFile {
                            pane: Some(code_pane),
                            path: preview_dir.join("note.md").display().to_string(),
                            mode: Some(tako_control::protocol::PreviewModeWire::Markdown),
                            direction: None,
                            focus: None,
                        },
                        PaneOrigin::Cli,
                    );
                    cx.notify();
                })
                .ok();
            wait(cx, 300).await;

            // 66c. プレビューのライブリロード（#233）。実 CLI の ON/OFF、OS イベント、
            //      300ms デバウンス、background 差し替えを一気通貫で検証する。
            let (reload_pane, reload_terminal, reload_tab) = window
                .update(cx, |app, _, _| {
                    (
                        app.previews.keys().next().copied().expect("preview がある"),
                        app.terminals.keys().next().copied().expect("terminal がある"),
                        app.workspace.active_tab_id(),
                    )
                })
                .unwrap_or_else(|_| fail("ライブリロード対象の取得"));
            for (action, expected) in [("off", false), ("on", true)] {
                let output = cli_output_bg(
                    cx,
                    &cli_path,
                    &ipc_endpoint,
                    &token,
                    reload_terminal,
                    reload_tab,
                    vec!["preview-reload".into(), action.into()],
                )
                .await
                .unwrap_or_else(|| fail(&format!("tako preview-reload {action} CLI 起動")));
                let stdout = String::from_utf8_lossy(&output.stdout);
                let state_ok = window
                    .update(cx, |app, _, _| app.preview_reload.enabled() == expected)
                    .unwrap_or(false);
                check(
                    output.status.success()
                        && stdout.contains(&format!(r#""enabled":{expected}"#))
                        && state_ok,
                    &format!("tako preview-reload {action} が core 状態へ反映"),
                );
            }
            let output = cli_output_bg(
                cx,
                &cli_path,
                &ipc_endpoint,
                &token,
                reload_terminal,
                reload_tab,
                vec!["preview-cache".into(), "512".into()],
            )
            .await
            .unwrap_or_else(|| fail("tako preview-cache 512 CLI 起動"));
            let stdout = String::from_utf8_lossy(&output.stdout);
            let cache_512 = window
                .update(cx, |app, _, _| app.preview_image_lru.budget_bytes() == 512 * 1024 * 1024)
                .unwrap_or(false);
            check(
                output.status.success() && stdout.contains(r#""max_mb":512"#) && cache_512,
                "tako preview-cache 512 が LRU 予算へ反映",
            );

            let long_markdown = |heading: &str| {
                let mut text = format!("# {heading}\n\n");
                for line in 0..100 {
                    text.push_str(&format!("- line {line}\n"));
                }
                text
            };
            std::fs::write(
                preview_dir.join("note.md"),
                long_markdown("Live baseline"),
            )
            .expect("スクロール可能な基準 Markdown を書ける");
            wait_for_preview_state(window, cx, Duration::from_secs(3), |app| {
                matches!(
                    app.previews.get(&reload_pane).map(|state| &state.content),
                    Some(preview::PreviewContent::Markdown(blocks))
                        if blocks.iter().any(|block| matches!(
                            block,
                            preview::MdBlock::Heading { spans, .. }
                                if spans.iter().any(|span| span.text == "Live baseline")
                        ))
                            && app.previews.get(&reload_pane).is_some_and(|state|
                                state.outline.items.first().is_some_and(|item|
                                    item.title == "Live baseline"))
                )
            })
            .await
            .unwrap_or_else(|| fail("スクロール保持用の基準 Markdown を反映"));
            window
                .update(cx, |app, preview_window, cx| {
                    app.preview_scroll_handles
                        .entry(reload_pane)
                        .or_default()
                        .set_offset(point(px(0.0), px(-48.0)));
                    preview_window.refresh();
                    cx.notify();
                })
                .ok();
            let _ = any.update(cx, |_, preview_window, cx| preview_window.draw(cx).clear());
            wait(cx, 50).await;
            let (mode_before, scroll_before, apply_before) = window
                .update(cx, |app, _, _| {
                    (
                        app.previews.get(&reload_pane).map(|state| state.mode),
                        app.preview_scroll_handles
                            .get(&reload_pane)
                            .map(|handle| f32::from(handle.offset().y))
                            .unwrap_or_default(),
                        app.preview_reload_apply_count,
                    )
                })
                .unwrap_or((None, 0.0, 0));
            check(
                mode_before == Some(preview::PreviewMode::Markdown),
                "連続 write 前は Markdown モード",
            );
            let mut final_write_at = std::time::Instant::now();
            for index in 0..6 {
                if index == 5 {
                    final_write_at = std::time::Instant::now();
                }
                std::fs::write(
                    preview_dir.join("note.md"),
                    long_markdown(&format!("Live reload {index}")),
                )
                .expect("連続 write を実行できる");
                if index < 5 {
                    wait(cx, 40).await;
                }
            }
            let live_delay = wait_for_preview_state(
                window,
                cx,
                Duration::from_secs(3),
                |app| {
                    matches!(
                        app.previews.get(&reload_pane).map(|state| &state.content),
                        Some(preview::PreviewContent::Markdown(blocks))
                            if blocks.iter().any(|block| matches!(
                                block,
                                preview::MdBlock::Heading { spans, .. }
                                    if spans.iter().any(|span| span.text == "Live reload 5")
                            ))
                                && app.previews.get(&reload_pane).is_some_and(|state|
                                    state.outline.items.first().is_some_and(|item|
                                        item.title == "Live reload 5"))
                    )
                },
            )
            .await
            .unwrap_or_else(|| fail("連続 write の最終内容が 3 秒以内に反映"));
            let final_delay = final_write_at.elapsed();
            // 遅延イベントが二重適用を起こさないことも観測する。
            wait(cx, 450).await;
            let (mode_after, scroll_after, apply_after) = window
                .update(cx, |app, _, _| {
                    (
                        app.previews.get(&reload_pane).map(|state| state.mode),
                        app.preview_scroll_handles
                            .get(&reload_pane)
                            .map(|handle| f32::from(handle.offset().y)),
                        app.preview_reload_apply_count,
                    )
                })
                .unwrap_or((None, None, 0));
            check(
                apply_after.saturating_sub(apply_before) == 1,
                "6 回の連続 write を 1 回だけ差し替える",
            );
            check(
                mode_after == mode_before
                    && scroll_after.is_some_and(|value| (value - scroll_before).abs() < 0.1),
                "ライブリロード後もモードとスクロール位置を保持",
            );
            check(
                live_delay <= Duration::from_millis(1500),
                "最終 write から 1.5 秒以内に反映",
            );
            println!(
                "TAKO_PREVIEW_RELOAD: writes=6 applies=1 delay_ms={} mode=markdown scroll_y={scroll_before:.1} outline=updated",
                final_delay.as_millis()
            );

            // 削除 / rename は旧パスに Error を出し、同パスへ戻せば自動復帰する。
            let note_path = preview_dir.join("note.md");
            let moved_path = preview_dir.join("note-renamed.md");
            std::fs::rename(&note_path, &moved_path).expect("表示中ファイルを rename できる");
            wait_for_preview_state(window, cx, Duration::from_secs(3), |app| {
                matches!(
                    app.previews.get(&reload_pane).map(|state| &state.content),
                    Some(preview::PreviewContent::Error(_))
                )
            })
            .await
            .unwrap_or_else(|| fail("削除 / rename 後に Error 表示へなる"));
            std::fs::rename(&moved_path, &note_path).expect("表示中パスへ戻せる");
            wait_for_preview_state(window, cx, Duration::from_secs(3), |app| {
                matches!(
                    app.previews.get(&reload_pane).map(|state| &state.content),
                    Some(preview::PreviewContent::Markdown(blocks))
                        if blocks.iter().any(|block| matches!(
                            block,
                            preview::MdBlock::Heading { spans, .. }
                                if spans.iter().any(|span| span.text == "Live reload 5")
                        ))
                )
            })
            .await
            .unwrap_or_else(|| fail("rename 元のパス復帰後に再読み込み"));

            // 1 MB 超は全体を読まず上限 + 1 byte で止め、既存の省略表示へ劣化。
            std::fs::write(&note_path, vec![b'x'; preview::MAX_BYTES + 128])
                .expect("巨大ファイルを書ける");
            wait_for_preview_state(window, cx, Duration::from_secs(3), |app| {
                app.previews
                    .get(&reload_pane)
                    .is_some_and(|state| state.truncated)
            })
            .await
            .unwrap_or_else(|| fail("巨大ファイルを省略表示"));
            std::fs::write(&note_path, "# Restored\n").expect("通常サイズへ戻せる");
            wait_for_preview_state(window, cx, Duration::from_secs(3), |app| {
                matches!(
                    app.previews.get(&reload_pane).map(|state| &state.content),
                    Some(preview::PreviewContent::Markdown(blocks))
                        if blocks.iter().any(|block| matches!(
                            block,
                            preview::MdBlock::Heading { spans, .. }
                                if spans.iter().any(|span| span.text == "Restored")
                        ))
                )
            })
            .await
            .unwrap_or_else(|| fail("巨大ファイル後に通常表示へ復帰"));

            // 編集モード中は外部変更を適用せず、FR-3.5 の Conflict へ接続する。
            let (edit_buffer_before, conflict_apply_before) = window
                .update(cx, |app, _, _| {
                    app.set_preview_editing_local(reload_pane, true)
                        .expect("編集モードを開始できる");
                    (
                        app.preview_edits
                            .get(&reload_pane)
                            .map(|edit| edit.buffer.text().to_string()),
                        app.preview_reload_apply_count,
                    )
                })
                .unwrap_or((None, 0));
            std::fs::write(&note_path, "# External conflict\n")
                .expect("編集中に外部変更できる");
            let conflict_delay = wait_for_preview_state(
                window,
                cx,
                Duration::from_secs(3),
                |app| {
                    app.preview_edits.get(&reload_pane).is_some_and(|edit| {
                        edit.save_status == Some(preview::SaveStatus::Conflict)
                    })
                },
            )
            .await
            .unwrap_or_else(|| fail("編集中の外部変更を Conflict 通知"));
            let conflict_preserved = window
                .update(cx, |app, _, _| {
                    let edit = app.preview_edits.get(&reload_pane);
                    let preserved = edit.map(|edit| edit.buffer.text().to_string())
                        == edit_buffer_before
                        && app.preview_reload_apply_count == conflict_apply_before;
                    app.set_preview_editing_local(reload_pane, false)
                        .expect("編集モードを終了できる");
                    preserved
                })
                .unwrap_or(false);
            check(conflict_preserved, "競合時は編集バッファを上書きしない");
            println!(
                "TAKO_PREVIEW_RELOAD: conflict_delay_ms={} buffer_preserved=true",
                conflict_delay.as_millis()
            );

            // 画像は生バイトを background で差し替え、#234 の zoom / pan を保持。
            let image_opened = window
                .update(cx, |app, _, cx| {
                    let opened = selftest_open(
                        app,
                        reload_pane.as_u64(),
                        image_path.display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Image),
                    )
                    .is_ok();
                    let view = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::PreviewView {
                            pane: Some(reload_pane.as_u64()),
                            zoom: Some(175.0),
                            zoom_in: false,
                            zoom_out: false,
                            reset: false,
                            page: None,
                            pan_x: Some(12.0),
                            pan_y: Some(8.0),
                        },
                        PaneOrigin::Cli,
                    )
                    .is_ok();
                    app.drain_pending_preview_loads(cx);
                    opened && view
                })
                .unwrap_or(false);
            check(
                image_opened,
                "画像プレビューをライブリロード対象で開く",
            );
            let image_before = window
                .update(cx, |app, _, _| {
                    app.previews.get(&reload_pane).and_then(|state| match &state.content {
                        preview::PreviewContent::Image(data) => Some(data.bytes.clone()),
                        _ => None,
                    })
                })
                .ok()
                .flatten()
                .unwrap_or_else(|| fail("画像更新前バイトの取得"));
            image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 40, 50, 255]))
                .save(&image_path)
                .expect("表示中 PNG を更新できる");
            wait_for_preview_state(window, cx, Duration::from_secs(3), |app| {
                app.previews.get(&reload_pane).is_some_and(|state| {
                    matches!(&state.content, preview::PreviewContent::Image(data)
                        if data.bytes != image_before)
                })
            })
            .await
            .unwrap_or_else(|| fail("PNG のライブリロード"));
            let image_view_preserved = window
                .update(cx, |app, _, _| {
                    app.preview_views.get(&reload_pane).is_some_and(|view| {
                        (view.zoom - 1.75).abs() < f32::EPSILON
                            && (view.pan_x - 12.0).abs() < f32::EPSILON
                            && (view.pan_y - 8.0).abs() < f32::EPSILON
                    })
                })
                .unwrap_or(false);
            check(image_view_preserved, "PNG 更新後も zoom / pan を保持");

            // PDF も #231/#234 の表示条件をキーに background 再ラスタライズする。
            write_test_pdf_with_outline(&pdf_path);
            window
                .update(cx, |app, _, cx| {
                    selftest_open(
                        app,
                        reload_pane.as_u64(),
                        pdf_path.display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Pdf),
                    )
                    .expect("PDF をライブリロード対象で開ける");
                    app.drain_pending_preview_loads(cx);
                })
                .ok();
            wait_for_preview_state(window, cx, Duration::from_secs(8), |app| {
                matches!(
                    app.previews.get(&reload_pane),
                    Some(state) if matches!(&state.content,
                        preview::PreviewContent::Pdf(data) if !data.pages.is_empty())
                        && state.outline.items.len() == 2
                )
            })
            .await
            .unwrap_or_else(|| fail("PDF 初回ラスタライズ"));
            write_test_pdf_with_text(&pdf_path, "Reloaded PDF");
            wait_for_preview_state(window, cx, Duration::from_secs(8), |app| {
                matches!(
                    app.previews.get(&reload_pane).map(|state| &state.content),
                    Some(preview::PreviewContent::Pdf(data))
                        if data.text_layers.iter().flatten().any(|line| line.text.contains("Reloaded PDF"))
                            && app.previews.get(&reload_pane).is_some_and(|state|
                                state.outline.is_empty())
                )
            })
            .await
            .unwrap_or_else(|| fail("PDF のライブ再ラスタライズ"));

            // 後続の編集 E2E は note.md を対象にするため Markdown へ戻す。
            window
                .update(cx, |app, _, cx| {
                    selftest_open(
                        app,
                        reload_pane.as_u64(),
                        note_path.display().to_string(),
                        Some(tako_control::protocol::PreviewModeWire::Markdown),
                    )
                    .expect("note.md へ戻せる");
                    cx.notify();
                })
                .ok();

            // 66d. 軽量編集（FR-3.5）: 実 CLI で開始 → 全文適用 → 保存し、子プロセスの
            //      成功終了を同期点として書き戻しを確認する。続けて MCP と同じ dispatch
            //      起点でも保存する。
            let (edit_preview_pane, edit_terminal_pane, edit_tab) = window
                .update(cx, |app, _, _| {
                    let preview = app.previews.keys().next().copied().expect("preview がある");
                    let terminal = app.terminals.keys().next().copied().expect("terminal がある");
                    let _ = app.workspace.active_tab_mut().tree_mut().focus(terminal);
                    (preview.as_u64(), terminal, app.workspace.active_tab_id())
                })
                .unwrap_or_else(|_| fail("編集 CLI の対象取得"));
            for (label, args) in [
                (
                    "start",
                    vec![
                        "edit".into(),
                        "start".into(),
                        "--pane".into(),
                        edit_preview_pane.to_string(),
                    ],
                ),
                (
                    "apply",
                    vec![
                        "edit".into(),
                        "apply".into(),
                        "--pane".into(),
                        edit_preview_pane.to_string(),
                        "CLI-日本語".into(),
                    ],
                ),
                (
                    "save",
                    vec![
                        "edit".into(),
                        "save".into(),
                        "--pane".into(),
                        edit_preview_pane.to_string(),
                    ],
                ),
            ] {
                let output = cli_output_bg(
                    cx,
                    &cli_path,
                    &ipc_endpoint,
                    &token,
                    edit_terminal_pane,
                    edit_tab,
                    args,
                )
                .await
                .unwrap_or_else(|| fail(&format!("tako edit {label} CLI 起動")));
                check(
                    output.status.success(),
                    &format!("tako edit {label} CLI 応答"),
                );
            }
            let cli_edit_ok = window
                .update(cx, |app, _, _| {
                    std::fs::read_to_string(preview_dir.join("note.md"))
                        .is_ok_and(|text| text == "CLI-日本語")
                        && app.preview_edits.values().any(|edit| {
                            !edit.dirty() && edit.buffer.text() == "CLI-日本語"
                        })
                })
                .unwrap_or(false);
            check(
                cli_edit_ok,
                "tako edit CLI の開始 / 適用 / 保存とファイル書き戻し",
            );
            let mcp_edit_ok = window
                .update(cx, |app, _, _| {
                    let applied = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::PreviewApply {
                            pane: Some(edit_preview_pane),
                            text: "MCP-日本語\n".into(),
                        },
                        PaneOrigin::Mcp,
                    );
                    let saved = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::PreviewSave {
                            pane: Some(edit_preview_pane),
                        },
                        PaneOrigin::Mcp,
                    );
                    applied.is_ok()
                        && saved.is_ok()
                        && std::fs::read_to_string(preview_dir.join("note.md"))
                            .is_ok_and(|text| text == "MCP-日本語\n")
                })
                .unwrap_or(false);
            check(mcp_edit_ok, "MCP dispatch の編集適用 / 保存と日本語書き戻し");
            // 後片付け: プレビューを閉じる（フォーカスはターミナルへ戻る）
            let cleaned = window
                .update(cx, |app, _, _| {
                    let pane = app.previews.keys().next().copied();
                    if let Some(pane) = pane {
                        let _ = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Close {
                                pane: Some(pane.as_u64()),
                                force: true,
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
                        let root_count = app.filetree.roots().len();
                        let has_tmp = app.filetree.roots().iter().any(|r| r.ends_with("tmp"));
                        let header_rows =
                            app.filetree.rows().iter().filter(|r| r.root).count();
                        root_count >= 2
                            && has_tmp
                            && header_rows == root_count
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
                            force: true,
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
                            force: true,
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

            // 73. TmuxOpen ビューペインのミラースクロール（#181）: `tako tmux open` で
            //      取り込んだ再アタッチ・ビューラッパーペインも #159 のローカルミラー +
            //      スクロールバー + CLI/MCP Scroll に乗る（backend_sessions に無いペインが
            //      直接ペイン扱いに落ち、alt screen（履歴なし）でスクロール不能だった
            //      実機バグの回帰検知）
            if has_tmux {
                let view_sock = format!("tako-selftest-view-{}", std::process::id());
                let created = std::process::Command::new("tmux")
                    .args(["-L", &view_sock, "new-session", "-d", "-s", "view-src"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                check(created, "TmuxOpen ミラー用 tmux セッション作成");
                // 履歴 200 行を積む（ミラー capture の対象）
                let _ = std::process::Command::new("tmux")
                    .args([
                        "-L",
                        &view_sock,
                        "send-keys",
                        "-t",
                        "=view-src:",
                        "seq 200",
                        "Enter",
                    ])
                    .status();
                wait(cx, 800).await;
                let view_pane_raw = window
                    .update(cx, |app, _, cx| {
                        let base = app.focused_pane().as_u64();
                        let opened = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::TmuxOpen {
                                socket: Some(view_sock.clone()),
                                session: "view-src".into(),
                                window: None,
                                pane: Some(base),
                                direction: Some(tako_control::protocol::Direction::Down),
                            },
                            PaneOrigin::Cli,
                        )
                        .expect("tmux open は成功する");
                        for (pane, options) in std::mem::take(&mut app.pending_attach) {
                            app.spawn_session(pane, options, cx)
                                .expect("取り込みペインの PTY 起動は成功する");
                        }
                        opened["pane"].as_u64().expect("pane が返る")
                    })
                    .unwrap_or(0);
                let view_pane = PaneId::from_raw(view_pane_raw);
                // attach クライアント成立 + 画面に seq 出力が映るまで待つ
                let mut view_ready = false;
                for _ in 0..25 {
                    wait(cx, 400).await;
                    view_ready = window
                        .update(cx, |app, _, _| {
                            app.terminals
                                .get(&view_pane)
                                .map(|s| {
                                    s.visible_lines().iter().any(|l| l.contains("200"))
                                })
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if view_ready {
                        break;
                    }
                }
                check(view_ready, "TmuxOpen ペインに外部セッションの画面が映る");
                // ホイール = ミラー表示 + スクロールバー表示（#159 と同じ機構に乗る）
                window
                    .update(cx, |app, win, cx| {
                        let center = app
                            .pane_text_areas
                            .iter()
                            .find(|(id, _)| *id == view_pane)
                            .map(|(_, b)| b.center())
                            .unwrap_or_default();
                        app.on_pane_scroll(
                            view_pane,
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
                let mut view_scrolled = false;
                for _ in 0..20 {
                    wait(cx, 300).await;
                    view_scrolled = window
                        .update(cx, |app, _, _| {
                            let mirrored = app
                                .scroll_ctls
                                .get(&view_pane)
                                .is_some_and(|c| c.mirror_scrolling());
                            // 合成行列がライブ viewport と異なる = 過去が見えている
                            let composed_differs = app
                                .terminals
                                .get(&view_pane)
                                .map(|s| s.screen(&app.theme))
                                .and_then(|screen| {
                                    let composed =
                                        app.compose_mirror_lines(view_pane, &screen)?;
                                    Some(composed.first()?.text != screen.lines.first()?.text)
                                })
                                .unwrap_or(false);
                            // バーは幾何計算の成立で判定する（実描画の text_area は
                            // ウィンドウが背面だと新設ペインに対して確立しないため、
                            // 固定サイズの仮領域を渡す。実 area での検証は 61c が担保）
                            let bar = app
                                .scrollbar_overlay(
                                    view_pane,
                                    Bounds {
                                        origin: point(px(0.0), px(0.0)),
                                        size: size(px(400.0), px(600.0)),
                                    },
                                )
                                .is_some();
                            mirrored && composed_differs && bar
                        })
                        .unwrap_or(false);
                    if view_scrolled {
                        break;
                    }
                }
                check(
                    view_scrolled,
                    "TmuxOpen ペインのホイールがミラー表示 + スクロールバーに乗る（#181）",
                );
                // CLI / MCP と共有の dispatch Scroll も同じミラーに効く（開発不変条件）
                let view_cli_scrolled = window
                    .update(cx, |app, _, _| {
                        let _ = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Scroll {
                                pane: Some(view_pane_raw),
                                to: Some(8),
                                delta: None,
                            },
                            PaneOrigin::Cli,
                        );
                        app.scroll_ctls
                            .get(&view_pane)
                            .map(|c| {
                                let pos = c
                                    .mirror
                                    .as_ref()
                                    .map(|m| m.position)
                                    .unwrap_or(c.pending_rows);
                                (pos - 8.0).abs() < 0.5
                            })
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                check(
                    view_cli_scrolled,
                    "dispatch Scroll が TmuxOpen ペインのミラー表示位置に効く（#181）",
                );
                // 後片付け: ペイン close（wrapper kill）+ サーバー kill
                let _ = window.update(cx, |app, _, cx| {
                    let _ = tako_control::dispatch(
                        app,
                        tako_control::protocol::Request::Close {
                            pane: Some(view_pane_raw),
                            force: true,
                        },
                        PaneOrigin::Cli,
                    );
                    cx.notify();
                });
                wait(cx, 500).await;
                let _ = std::process::Command::new("tmux")
                    .args(["-L", &view_sock, "kill-server"])
                    .status();
            } else {
                eprintln!("（tmux 不在のため項目 73 をスキップ）");
            }

            // 74. worker_status の IPC 応答（#181 → #168 で OffloadJob へ一本化）:
            //      OrchestratorWorkerStatus は IPC ループで prepare_offload（UI スレッドで
            //      文脈収集）→ OffloadJob::run（background executor）に分離して応答する
            //      （claude CLI 起動 500〜1100ms の UI 専有 = スクロールのカクつき根治）。
            //      分離後も実 CLI → IPC 経由で応答が返ることを機械検証する
            {
                let ws_pane = window
                    .update(cx, |app, _, _| app.focused_pane().as_u64())
                    .unwrap_or(0);
                let ws_out = std::env::temp_dir()
                    .join(format!("tako-selftest-ws-{}.json", std::process::id()));
                let _ = std::fs::remove_file(&ws_out);
                press(any, cx, "ctrl-u");
                type_text(
                    any,
                    cx,
                    &format!(
                        "{cli} orchestrator status --pane {ws_pane} > {}",
                        ws_out.display()
                    ),
                    true,
                );
                let mut ws_ok = false;
                for _ in 0..25 {
                    wait(cx, 400).await;
                    ws_ok = std::fs::read_to_string(&ws_out)
                        .map(|s| s.contains("\"status\""))
                        .unwrap_or(false);
                    if ws_ok {
                        break;
                    }
                }
                check(
                    ws_ok,
                    "worker_status が IPC（background 合成）経由で応答する（#181）",
                );
                let _ = std::fs::remove_file(&ws_out);
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
                                focus: None,
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
                            tako_control::protocol::Request::Close { pane: Some(pane), force: true },
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
                            focus: None,
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
                        tako_control::protocol::Request::Close { pane: Some(p2), force: true },
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
                                focus: None,
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
                            force: true,
                        },
                        PaneOrigin::Cli,
                    );
                    png_ok && jpg_ok && has_image
                })
                .unwrap_or(false);
            check(img_ok, "画像プレビュー（FR-3.10。PNG / JPEG の OpenFile と list 公開）");
            let _ = std::fs::remove_dir_all(&img_dir);

            // 69b. Issue #64: 全角半角混在行で半角グリフが折り返しで消えないこと。
            //      根因: グループ div の幅（セル幅 × セル数）を GPUI が wrap_width として
            //      テキストを折り返し、折り返された文字（「ターミナルUI」の I、
            //      「Fable 5 + max」の max）が行 div の overflow_hidden の外へ出て消える。
            //      既知失敗の PDF（70）で exit する前に検証するためここに置く
            press(any, cx, "ctrl-u");
            type_text(
                any,
                cx,
                "echo '⏺ Fable 5 + max'; echo 'ターミナルUI'",
                true,
            );
            let mut issue64 = (false, false, false);
            for _ in 0..8 {
                wait(cx, 800).await;
                issue64 = window
                    .update(cx, |app, win, _| {
                        // a. 根因の実在証明: グリッド幅ちょうどの wrap_width で ASCII を
                        //    シェイプすると折り返しが起きる（旧実装はこれが文字消失だった）
                        let ts = win.text_system().clone();
                        let family = app.theme.font_family.clone();
                        let font = Font {
                            family: SharedString::from(family.clone()),
                            ..gpui::font(family)
                        };
                        let font_id = ts.resolve_font(&font);
                        let fs = px(app.theme.font_size);
                        let cw = ts.advance(font_id, fs, 'M').ok()?.width;
                        let wraps = |text: &str| {
                            let run = TextRun {
                                len: text.len(),
                                font: font.clone(),
                                color: gpui::black(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let cols = text.chars().count();
                            ts.shape_text(
                                SharedString::from(text.to_string()),
                                fs,
                                &[run],
                                Some(cw * cols as f32),
                                None,
                            )
                            .ok()
                            .and_then(|lines| {
                                lines.first().map(|l| l.wrap_boundaries().len())
                            })
                            .unwrap_or(0)
                        };
                        let repro = wraps("Fable 5 + max") >= 1 && wraps("UI") >= 1;

                        // b. 修正の構造検証: 行 div は whitespace_nowrap
                        //    （wrap_width = None になり折り返し経路が呼ばれない）
                        let pane = app.focused_pane();
                        let mut lines = app.terminal_screen_lines(pane, false);
                        let nowrap = !lines.is_empty()
                            && lines.iter_mut().all(|d| {
                                d.style().text.white_space == Some(gpui::WhiteSpace::Nowrap)
                            });

                        // c. セル幅不一致グリフの隔離: ⏺（テーマフォント外）は単独
                        //    チャンク、ASCII 連続はグループ維持（#39 の要素数削減）、
                        //    全角は個別 div のまま
                        let screen = app.terminals.get(&pane)?.screen(&app.theme);
                        let line = screen
                            .lines
                            .iter()
                            .find(|l| l.text.starts_with("⏺ Fable 5 + max"))?;
                        let infos = app.line_char_infos(line);
                        let chunks = chunk_line_chars(&infos);
                        let mark = infos.iter().position(|i| i.ch == '⏺')?;
                        let mark_solo =
                            chunks.iter().any(|c| c.start == mark && c.end == mark + 1);
                        let f = infos.iter().position(|i| i.ch == 'F')?;
                        let x = infos.iter().rposition(|i| i.ch == 'x')?;
                        let ascii_grouped = chunks.iter().any(|c| c.start <= f && x < c.end);
                        let line2 = screen
                            .lines
                            .iter()
                            .find(|l| l.text.starts_with("ターミナルUI"))?;
                        let infos2 = app.line_char_infos(line2);
                        let chunks2 = chunk_line_chars(&infos2);
                        let ta = infos2.iter().position(|i| i.ch == 'タ')?;
                        let ta_solo = chunks2
                            .iter()
                            .any(|c| c.start == ta && c.end == ta + 1 && c.cols == 2);
                        let u = infos2.iter().position(|i| i.ch == 'U')?;
                        let ui_grouped = chunks2.iter().any(|c| c.start <= u && u + 2 <= c.end);
                        let snap_judge =
                            !app.glyph_snaps_to_cell('⏺') && app.glyph_snaps_to_cell('a');
                        Some((
                            repro,
                            nowrap,
                            mark_solo && ascii_grouped && ta_solo && ui_grouped && snap_judge,
                        ))
                    })
                    .ok()
                    .flatten()
                    .unwrap_or((false, false, false));
                if issue64.0 && issue64.1 && issue64.2 {
                    break;
                }
            }
            check(
                issue64.0,
                "半角消失の根因が実在（グリッド幅シェイプで折り返し発生）",
            );
            check(issue64.1, "行 div の whitespace_nowrap（折り返しの構造的禁止）");
            check(issue64.2, "セル幅不一致グリフの隔離 + ASCII グループ化維持");

            // 69c. ターミナルリンク（#153）: 実 PTY に絶対 / ~/ / cwd 相対パスを表示し、
            //      画面スナップショットからの検出と cmd+クリック相当の MouseDown を通す。
            //      ファイルは OpenFile プレビュー、ディレクトリは Split + PTY 起動まで実測する。
            let link_dir = std::path::PathBuf::from("/private/tmp")
                .join(format!("tako-selftest-link-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&link_dir);
            std::fs::create_dir_all(link_dir.join("sub"))
                .expect("リンク用一時ディレクトリを作れる");
            let link_file = link_dir.join("sub/relative.txt");
            let absolute_file = link_dir.join("absolute.txt");
            std::fs::write(&link_file, "link selftest\n").unwrap();
            std::fs::write(&absolute_file, "absolute link selftest\n").unwrap();
            let link_command = format!(
                "cd {} && printf '%s\\n' LINK_SELFTEST {} '~/' sub/relative.txt {}",
                shell_escape(&link_dir),
                shell_escape(&absolute_file),
                shell_escape(&link_dir),
            );
            press(any, cx, "ctrl-u");
            type_text(any, cx, &link_command, true);
            let mut link_screen_ready = false;
            for _ in 0..12 {
                wait(cx, 300).await;
                link_screen_ready = window
                    .update(cx, |app, _, _| {
                        app.terminals
                            .get(&app.focused_pane())
                            .is_some_and(|session| {
                                session.cwd() == Some(link_dir.as_path())
                                    && session
                                        .visible_lines()
                                        .iter()
                                        .any(|line| line.trim() == "LINK_SELFTEST")
                            })
                    })
                    .unwrap_or(false);
                if link_screen_ready {
                    break;
                }
            }
            check(link_screen_ready, "リンク検証テキストを実 PTY 画面へ表示");

            let (absolute_ok, home_ok, relative_ok, cwd_ok) = window
                .update(cx, |app, _, _| {
                    let base = app.focused_pane();
                    app.refresh_pane_links(base);
                    let links = app.pane_links.get(&base).cloned().unwrap_or_default();
                    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                    let has_absolute = links.iter().any(|link| {
                        link.kind == tako_core::LinkKind::Path
                            && std::path::Path::new(&link.target) == absolute_file
                    });
                    let has_relative = links.iter().any(|link| {
                        link.kind == tako_core::LinkKind::Path
                            && std::path::Path::new(&link.target) == link_file
                    });
                    let has_home = home.is_some_and(|home| {
                        links.iter().any(|link| {
                            link.kind == tako_core::LinkKind::Path
                                && std::path::Path::new(&link.target) == home
                        })
                    });
                    let cwd_matches = app
                        .terminals
                        .get(&base)
                        .and_then(|session| session.cwd())
                        == Some(link_dir.as_path());
                    (has_absolute, has_home, has_relative, cwd_matches)
                })
                .unwrap_or((false, false, false, false));
            check(absolute_ok, "絶対パスを実画面から検出・解決");
            check(home_ok, "~/ 起点パスを実画面から検出・解決");
            check(relative_ok, "cwd 相対パスを実画面から検出・解決");
            check(cwd_ok, "リンク解決 cwd が実 PTY の OSC 7 と一致");

            // 任意のピクセル検証停止点。通常の self-test では待機しない。
            // 実画面の検出結果とセル座標から cmd ホバーと同じ更新関数を通し、外部の
            // screencapture が装飾前後を同一ウィンドウで取得できるようにする。
            if std::env::var_os("TAKO_SELF_TEST_LINK_VISUAL").is_some() {
                println!("TAKO_LINK_VISUAL_BASELINE_READY");
                wait(cx, 15_000).await;
                let visual_hovered = window
                    .update(cx, |app, win, cx| {
                        let base = app.focused_pane();
                        app.refresh_pane_links(base);
                        let link = app
                            .pane_links
                            .get(&base)?
                            .iter()
                            .find(|link| std::path::Path::new(&link.target) == link_file)?
                            .clone();
                        let &(row, start, _) = link.spans.first()?;
                        let area = app
                            .pane_text_areas
                            .iter()
                            .find(|(pane, _)| *pane == base)
                            .map(|(_, area)| *area)?;
                        let cell = app.cell_size_for_pane(base)?;
                        let position = point(
                            area.origin.x + cell.width * (start as f32 + 0.5),
                            area.origin.y + cell.height * (row as f32 + 0.5),
                        );
                        app.update_hovered_link_at(position, true, win, cx);
                        app.hovered_link
                            .as_ref()
                            .is_some_and(|hovered| hovered.contains(base, row, start))
                            .then_some(())
                    })
                    .ok()
                    .flatten()
                    .is_some();
                check(visual_hovered, "ピクセル検証用 cmd ホバー状態を構築");
                println!("TAKO_LINK_VISUAL_HOVER_READY");
                wait(cx, 60_000).await;
                let _ = window.update(cx, |app, _, cx| {
                    app.hovered_link = None;
                    cx.notify();
                });
            }

            let (file_click_ok, directory_click_ok) = window
                .update(cx, |app, win, cx| {
                    let base = app.focused_pane();
                    app.refresh_pane_links(base);
                    let links = app.pane_links.get(&base)?.clone();

                    let click = |app: &mut TakoApp,
                                 link: &tako_core::DetectedLink,
                                 win: &mut Window,
                                 cx: &mut Context<TakoApp>| {
                        let &(row, start, _) = link.spans.first()?;
                        let area = app
                            .pane_text_areas
                            .iter()
                            .find(|(pane, _)| *pane == base)
                            .map(|(_, area)| *area)?;
                        let cell = app.cell_size_for_pane(base)?;
                        let position = point(
                            area.origin.x + cell.width * (start as f32 + 0.5),
                            area.origin.y + cell.height * (row as f32 + 0.5),
                        );
                        app.hovered_link = None;
                        app.on_pane_mouse_down(
                            base,
                            &MouseDownEvent {
                                button: MouseButton::Left,
                                position,
                                modifiers: Modifiers {
                                    platform: true,
                                    ..Modifiers::default()
                                },
                                click_count: 1,
                                first_mouse: false,
                            },
                            win,
                            cx,
                        );
                        Some(())
                    };

                    let file_link = links
                        .iter()
                        .find(|link| std::path::Path::new(&link.target) == link_file)?;
                    click(app, file_link, win, cx)?;
                    let preview = app.previews.iter().find_map(|(pane, state)| {
                        (state.path == link_file).then_some(*pane)
                    });
                    let file_click_ok = preview.is_some();
                    if let Some(preview) = preview {
                        let _ = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Close {
                                pane: Some(preview.as_u64()),
                                force: true,
                            },
                            PaneOrigin::Cli,
                        );
                    }

                    app.refresh_pane_links(base);
                    let dir_link = app
                        .pane_links
                        .get(&base)?
                        .iter()
                        .find(|link| std::path::Path::new(&link.target) == link_dir)?
                        .clone();
                    let terminals_before: std::collections::HashSet<_> =
                        app.terminals.keys().copied().collect();
                    click(app, &dir_link, win, cx)?;
                    let directory_pane = app.terminals.iter().find_map(|(pane, session)| {
                        (!terminals_before.contains(pane)
                            && session.cwd() == Some(link_dir.as_path()))
                        .then_some(*pane)
                    });
                    let directory_click_ok = directory_pane.is_some();
                    if let Some(pane) = directory_pane {
                        let _ = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Close {
                                pane: Some(pane.as_u64()),
                                force: true,
                            },
                            PaneOrigin::Cli,
                        );
                    }
                    Some((file_click_ok, directory_click_ok))
                })
                .ok()
                .flatten()
                .unwrap_or((false, false));
            check(file_click_ok, "cmd+クリックでファイルを分割プレビュー表示");
            check(
                directory_click_ok,
                "cmd+クリックでディレクトリを分割し PTY を cwd 付きで起動",
            );
            let _ = std::fs::remove_dir_all(&link_dir);

            // 70. PDF プレビュー（FR-3.4 macOS）: dispatch OpenFile で PDF を開き、
            //     Pdf モードで Core Graphics レンダリングされたページが表示される
            #[cfg(target_os = "macos")]
            {
                let pdf_dir =
                    std::env::temp_dir().join(format!("tako-selftest-pdf-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&pdf_dir);
                std::fs::create_dir_all(&pdf_dir).expect("一時ディレクトリを作れる");
                let pdf_path = pdf_dir.join("test.pdf");
                write_test_pdf(&pdf_path);
                // PDF は background 読み込み（Issue #168）: open 応答は即返り、
                // 内容はポーリングで完了を待ってから検証する
                let pdf_pane = window
                    .update(cx, |app, _, cx| {
                        let base = app.focused_pane().as_u64();
                        let r = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::OpenFile {
                                pane: Some(base),
                                path: pdf_path.display().to_string(),
                                mode: None,
                                direction: None,
                                focus: None,
                            },
                            PaneOrigin::Cli,
                        )
                        .expect("PDF を開ける");
                        app.drain_pending_preview_loads(cx);
                        let pane_id = r["pane"].as_u64().expect("pane が返る");
                        (r["mode"].as_str() == Some("pdf")).then_some(pane_id)
                    })
                    .ok()
                    .flatten();
                let mut pdf_ok = false;
                if let Some(pane_id) = pdf_pane {
                    for _ in 0..30 {
                        wait(cx, 200).await;
                        pdf_ok = window
                            .update(cx, |app, _, _| {
                                app.previews.iter().any(|(pid, p)| {
                                    pid.as_u64() == pane_id
                                        && matches!(
                                            &p.content,
                                            preview::PreviewContent::Pdf(d)
                                                if d.total_pages == 1
                                                    && d.pages.len() == 1
                                                    && !d.pages[0].is_empty()
                                        )
                                })
                            })
                            .unwrap_or(false);
                        if pdf_ok {
                            break;
                        }
                    }
                    let _ = window.update(cx, |app, _, _| {
                        let _ = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Close {
                                pane: Some(pane_id),
                                force: true,
                            },
                            PaneOrigin::Cli,
                        );
                    });
                }
                check(pdf_ok, "PDF プレビュー（FR-3.4。Core Graphics レンダリング）");
                let _ = std::fs::remove_dir_all(&pdf_dir);
            }

            // 71. Web ビューペイン e2e（FR-3.8 / #155）: wry ネイティブ webview の
            //     open → list → read（タイトル追跡）→ navigate → eval → hide → show → close。
            //     ページは data: URL（外部ネットワーク不要）。dispatch 直呼び = CLI / MCP と
            //     同一経路（開発不変条件。引数変換は mcp.rs / tako-cli の単体テストが担保）
            {
                #[allow(clippy::too_many_arguments)]
                fn web_req(
                    action: &str,
                    url: Option<&str>,
                    id: Option<u64>,
                    to: Option<&str>,
                    js: Option<&str>,
                    token: Option<u64>,
                ) -> tako_control::protocol::Request {
                    tako_control::protocol::Request::Web {
                        action: action.into(),
                        url: url.map(String::from),
                        id,
                        pane: None,
                        direction: None,
                        to: to.map(String::from),
                        js: js.map(String::from),
                        token,
                        focus: None,
                    }
                }
                let base_panes = window
                    .update(cx, |app, _, _| app.workspace.active_tab().tree().len())
                    .unwrap_or(0);
                // open: フォーカスペインを分割して wry WebView を生成
                let opened = window
                    .update(cx, |app, _, _cx| {
                        tako_control::dispatch(
                            app,
                            web_req(
                                "open",
                                Some("data:text/html,<title>tako-wv-test</title><h1>hi</h1>"),
                                None,
                                None,
                                None,
                                None,
                            ),
                            PaneOrigin::Cli,
                        )
                        .ok()
                        .and_then(|v| Some((v["id"].as_u64()?, v["pane"].as_u64()?)))
                    })
                    .ok()
                    .flatten();
                let Some((web_id, web_pane)) = opened else {
                    fail("Web ビュー open（wry 生成）");
                };
                // list: id・ペイン対応・URL が載る
                let listed = window
                    .update(cx, |app, _, _cx| {
                        let r = tako_control::dispatch(
                            app,
                            web_req("list", None, None, None, None, None),
                            PaneOrigin::Cli,
                        )
                        .ok()?;
                        let arr = r.as_array()?;
                        Some(arr.iter().any(|e| {
                            e["id"].as_u64() == Some(web_id)
                                && e["pane"].as_u64() == Some(web_pane)
                        }))
                    })
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                check(listed, "Web ビュー list（id / ペイン対応）");
                // read: 実ページからのタイトル追跡（初期化スクリプト → ipc 往復）を待つ
                let mut title_ok = false;
                for _ in 0..25 {
                    wait(cx, 200).await;
                    title_ok = window
                        .update(cx, |app, _, _cx| {
                            let r = tako_control::dispatch(
                                app,
                                web_req("read", None, Some(web_id), None, None, None),
                                PaneOrigin::Cli,
                            )
                            .ok()?;
                            Some(r["title"].as_str() == Some("tako-wv-test"))
                        })
                        .ok()
                        .flatten()
                        .unwrap_or(false);
                    if title_ok {
                        break;
                    }
                }
                if !title_ok {
                    // 切り分け診断: ipc（タイトル追跡）不達時に read の生値と
                    // evaluate_script_with_callback の生存を出力してから fail する
                    let diag = window
                        .update(cx, |app, _, _cx| {
                            let read = tako_control::dispatch(
                                app,
                                web_req("read", None, Some(web_id), None, None, None),
                                PaneOrigin::Cli,
                            );
                            let ev = tako_control::dispatch(
                                app,
                                web_req(
                                    "eval",
                                    None,
                                    Some(web_id),
                                    None,
                                    Some("document.title"),
                                    None,
                                ),
                                PaneOrigin::Cli,
                            );
                            format!("read={read:?} eval={ev:?}")
                        })
                        .unwrap_or_default();
                    println!("TAKO_WV_DIAG1: {diag}");
                    wait(cx, 2000).await;
                    let diag2 = window
                        .update(cx, |app, _, _cx| {
                            let r = tako_control::dispatch(
                                app,
                                web_req("eval_result", None, Some(web_id), None, None, Some(1)),
                                PaneOrigin::Cli,
                            );
                            format!("eval_result(token=1)={r:?}")
                        })
                        .unwrap_or_default();
                    println!("TAKO_WV_DIAG2: {diag2}");
                }
                check(title_ok, "Web ビュー read（実ページのタイトル追跡）");
                // navigate: URL 遷移でタイトルが変わる
                let _ = window.update(cx, |app, _, _cx| {
                    let _ = tako_control::dispatch(
                        app,
                        web_req(
                            "navigate",
                            None,
                            Some(web_id),
                            Some("data:text/html,<title>tako-wv-2</title>ok"),
                            None,
                            None,
                        ),
                        PaneOrigin::Cli,
                    );
                });
                let mut nav_ok = false;
                for _ in 0..25 {
                    wait(cx, 200).await;
                    nav_ok = window
                        .update(cx, |app, _, _cx| {
                            let r = tako_control::dispatch(
                                app,
                                web_req("read", None, Some(web_id), None, None, None),
                                PaneOrigin::Cli,
                            )
                            .ok()?;
                            Some(r["title"].as_str() == Some("tako-wv-2"))
                        })
                        .ok()
                        .flatten()
                        .unwrap_or(false);
                    if nav_ok {
                        break;
                    }
                }
                check(nav_ok, "Web ビュー navigate（URL 遷移 + タイトル更新）");
                // eval → eval_result: JS 評価（AI の画面操作経路）
                let eval_token = window
                    .update(cx, |app, _, _cx| {
                        tako_control::dispatch(
                            app,
                            web_req("eval", None, Some(web_id), None, Some("1+2"), None),
                            PaneOrigin::Cli,
                        )
                        .ok()
                        .and_then(|v| v["token"].as_u64())
                    })
                    .ok()
                    .flatten();
                let Some(eval_token) = eval_token else {
                    fail("Web ビュー eval 発行");
                };
                let mut eval_ok = false;
                for _ in 0..25 {
                    wait(cx, 200).await;
                    eval_ok = window
                        .update(cx, |app, _, _cx| {
                            let r = tako_control::dispatch(
                                app,
                                web_req(
                                    "eval_result",
                                    None,
                                    Some(web_id),
                                    None,
                                    None,
                                    Some(eval_token),
                                ),
                                PaneOrigin::Cli,
                            )
                            .ok()?;
                            Some(r["result"].as_i64() == Some(3))
                        })
                        .ok()
                        .flatten()
                        .unwrap_or(false);
                    if eval_ok {
                        break;
                    }
                }
                check(eval_ok, "Web ビュー eval → eval_result（JS 評価）");
                // hide: dock 退避（ページは生存、ペインは閉じる）
                let hidden = window
                    .update(cx, |app, _, _cx| {
                        let _ = tako_control::dispatch(
                            app,
                            web_req("hide", None, Some(web_id), None, None, None),
                            PaneOrigin::Cli,
                        )
                        .ok()?;
                        let r = tako_control::dispatch(
                            app,
                            web_req("list", None, None, None, None, None),
                            PaneOrigin::Cli,
                        )
                        .ok()?;
                        let arr = r.as_array()?;
                        Some(
                            arr.iter().any(|e| {
                                e["id"].as_u64() == Some(web_id) && e["pane"].is_null()
                            }) && app.workspace.active_tab().tree().len() == base_panes,
                        )
                    })
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                check(hidden, "Web ビュー hide（dock 退避 + ペイン後始末）");
                // show: dock からワンクリック復帰。ページ状態（タイトル）が維持されている
                let shown_alive = window
                    .update(cx, |app, _, _cx| {
                        let r = tako_control::dispatch(
                            app,
                            web_req("show", None, Some(web_id), None, None, None),
                            PaneOrigin::Cli,
                        )
                        .ok()?;
                        let shown_pane = r["pane"].as_u64()?;
                        let read = tako_control::dispatch(
                            app,
                            web_req("read", None, Some(web_id), None, None, None),
                            PaneOrigin::Cli,
                        )
                        .ok()?;
                        Some(
                            shown_pane != web_pane
                                && read["title"].as_str() == Some("tako-wv-2"),
                        )
                    })
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                check(
                    shown_alive,
                    "Web ビュー show（復帰後もページ状態が維持 = インスタンス保持）",
                );
                // close: 完全破棄 + ペイン数が元に戻る
                let closed = window
                    .update(cx, |app, _, _cx| {
                        let _ = tako_control::dispatch(
                            app,
                            web_req("close", None, Some(web_id), None, None, None),
                            PaneOrigin::Cli,
                        )
                        .ok()?;
                        let r = tako_control::dispatch(
                            app,
                            web_req("list", None, None, None, None, None),
                            PaneOrigin::Cli,
                        )
                        .ok()?;
                        Some(
                            r.as_array().map(|a| a.is_empty()).unwrap_or(false)
                                && app.workspace.active_tab().tree().len() == base_panes,
                        )
                    })
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                check(closed, "Web ビュー close（完全破棄 + ペイン後始末）");
            }

            // 72. worker spawn レイアウトエンジン（#165）: OrchestratorSpawn の配置が
            //     master-reserved / grid（既定）で行われ、worker close 後に worker 領域内
            //     だけがリフローされ、ユーザー由来ペインの矩形が不変であることを
            //     rect（単位矩形の比率）で機械検証する。dispatch 直呼び = CLI / MCP と
            //     同一経路（開発不変条件）。spawn の cwd 解決用に projects.yaml へ
            //     一時プロジェクトを登録し、終了時に削除する。worker エージェント CLI の
            //     起動完了は待たない（レイアウト検証には無関係。prompt は空文字のため
            //     エージェントが起動しても API 呼び出しは発生しない。事前信頼の書き込みが
            //     ~/.claude.json に一時 dir の trust を 1 エントリ残しうるが、実在しない
            //     パスとして無視される）
            {
                fn rect_close(r: tako_core::Rect, x: f32, y: f32, w: f32, h: f32) -> bool {
                    (r.x - x).abs() < 1e-3
                        && (r.y - y).abs() < 1e-3
                        && (r.width - w).abs() < 1e-3
                        && (r.height - h).abs() < 1e-3
                }
                fn spawn_req(pane: u64, label: &str) -> tako_control::protocol::Request {
                    tako_control::protocol::Request::OrchestratorSpawn {
                        project: "tako-selftest-165".into(),
                        prompt: String::new(),
                        label: Some(label.into()),
                        model: None,
                        effort: None,
                        pane: Some(pane),
                        tab: None,
                        caller_role: None,
                        agent: None,
                        caller_pid: None,
                        task_type: None,
                    }
                }

                let scratch = std::env::temp_dir()
                    .join(format!("tako-selftest-165-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&scratch);
                let registered = (|| -> Result<(), String> {
                    let mut config = tako_control::orchestrator::ProjectsConfig::load()?;
                    config.add(
                        "tako-selftest-165".to_string(),
                        scratch.display().to_string(),
                        Some("selftest #165（自動削除される）".into()),
                    );
                    config.save()
                })();
                check(registered.is_ok(), "spawn レイアウト: 一時プロジェクト登録");

                // 専用タブ（root = master 役）+ master の下にユーザー由来ペイン
                let ids = window
                    .update(cx, |app, _, _cx| {
                        let r = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::TabNew {
                                title: Some("spawn-layout".into()),
                                focus: None,
                            },
                            PaneOrigin::Cli,
                        )
                        .ok()?;
                        let tab = r["tab"].as_u64()?;
                        let master = app
                            .workspace
                            .get_tab(TabId::from_raw(tab))?
                            .tree()
                            .focused()
                            .as_u64();
                        let user = tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Split {
                                pane: Some(master),
                                tab: None,
                                direction: Some(tako_control::protocol::Direction::Down),
                                ratio: None,
                                command: None,
                                cwd: None,
                                focus: None,
                            },
                            PaneOrigin::User,
                        )
                        .ok()?["pane"]
                            .as_u64()?;
                        Some((tab, master, user))
                    })
                    .ok()
                    .flatten();
                let Some((lt_tab, lt_master, lt_user)) = ids else {
                    fail("spawn レイアウト: 専用タブ + ユーザーペイン準備");
                };
                let rect_of = |cx: &mut AsyncApp, pane: u64| -> Option<tako_core::Rect> {
                    window
                        .update(cx, |app, _, _cx| {
                            app.workspace
                                .get_tab(TabId::from_raw(lt_tab))?
                                .tree()
                                .layout(tako_core::Rect::UNIT)
                                .into_iter()
                                .find(|(id, _)| id.as_u64() == pane)
                                .map(|(_, r)| r)
                        })
                        .ok()
                        .flatten()
                };
                let user_rect0 = rect_of(cx, lt_user);
                check(
                    user_rect0.is_some_and(|r| rect_close(r, 0.0, 0.5, 1.0, 0.5)),
                    "spawn レイアウト: ユーザーペイン初期配置（下半分）",
                );

                // spawn 1〜4 体。各回 dispatch の戻りから pane_id を得て rect を検証する
                let mut workers = Vec::new();
                for label in ["w1", "w2", "w3", "w4"] {
                    let spawned = window
                        .update(cx, |app, _, _cx| {
                            tako_control::dispatch(
                                app,
                                spawn_req(lt_master, label),
                                PaneOrigin::Mcp,
                            )
                            .ok()
                            .and_then(|v| v["pane_id"].as_u64())
                        })
                        .ok()
                        .flatten();
                    let Some(id) = spawned else {
                        fail(&format!("spawn レイアウト: {label} の spawn"));
                    };
                    workers.push(id);
                }
                let (w1, w2, w3, w4) = (workers[0], workers[1], workers[2], workers[3]);
                // master は上半分の中で左 50% を維持、右上 1/4 が worker 領域の十字四分割
                check(
                    rect_of(cx, lt_master).is_some_and(|r| rect_close(r, 0.0, 0.0, 0.5, 0.5)),
                    "spawn レイアウト: 4 spawn 後も master は左 50% を維持",
                );
                for (id, x, y, name) in [
                    (w1, 0.5, 0.0, "w1 左上"),
                    (w2, 0.5, 0.25, "w2 左下"),
                    (w3, 0.75, 0.0, "w3 右上"),
                    (w4, 0.75, 0.25, "w4 右下"),
                ] {
                    check(
                        rect_of(cx, id).is_some_and(|r| rect_close(r, x, y, 0.25, 0.25)),
                        &format!("spawn レイアウト: grid 十字四分割の {name}"),
                    );
                }
                check(
                    rect_of(cx, lt_user) == user_rect0,
                    "spawn レイアウト: 4 spawn 後もユーザーペインの矩形は不変",
                );

                // w2 を close → worker 領域内だけがリフローされる（dispatch Close 経路）
                let closed = window
                    .update(cx, |app, _, _cx| {
                        tako_control::dispatch(
                            app,
                            tako_control::protocol::Request::Close {
                                pane: Some(w2),
                                force: true,
                            },
                            PaneOrigin::Mcp,
                        )
                        .is_ok()
                    })
                    .unwrap_or(false);
                check(closed, "spawn レイアウト: w2 の close");
                for (id, x, y, w, h, name) in [
                    (w1, 0.5, 0.0, 0.25, 0.25, "w1 左上"),
                    (w3, 0.5, 0.25, 0.25, 0.25, "w3 左下"),
                    (w4, 0.75, 0.0, 0.25, 0.5, "w4 右列全高"),
                ] {
                    check(
                        rect_of(cx, id).is_some_and(|r| rect_close(r, x, y, w, h)),
                        &format!("spawn レイアウト: close 後リフローの {name}"),
                    );
                }
                check(
                    rect_of(cx, lt_master).is_some_and(|r| rect_close(r, 0.0, 0.0, 0.5, 0.5))
                        && rect_of(cx, lt_user) == user_rect0,
                    "spawn レイアウト: close 後も master とユーザーペインは不変",
                );

                // 後始末: タブごと閉じて worker PTY を破棄 + 一時プロジェクト削除
                let _ = window.update(cx, |app, _, cx| {
                    app.remove_tab(TabId::from_raw(lt_tab), cx);
                });
                let _ = (|| -> Result<(), String> {
                    let mut config = tako_control::orchestrator::ProjectsConfig::load()?;
                    config.remove("tako-selftest-165");
                    config.save()
                })();
                let _ = std::fs::remove_dir_all(&scratch);
            }

            // 73. 確認ダイアログ（Issue #172）: × ボタン close の確認ダイアログが
            //     正しく表示・承認・キャンセルされること。cmd+クリック相当のスキップ。
            //     設定 OFF で即クローズ。
            {
                // ペインを 2 つにして対象を作る
                type_text(
                    any,
                    cx,
                    &format!("{cli} split --right --focus >/dev/null"),
                    true,
                );
                wait(cx, 1500).await;

                // 73a. 確認ダイアログの表示 + Esc キャンセル
                let esc_ok = window
                    .update(cx, |app, _, cx| {
                        let before = app.workspace.active_tab().tree().len();
                        let target = app.focused_pane();
                        app.confirm_close = true;
                        app.close_pane_with_confirm(target, false, cx);
                        let dialog_shown = app.pending_close_confirm
                            == Some(CloseConfirmTarget::Pane(target));
                        let not_closed = app.workspace.active_tab().tree().len() == before;
                        app.close_confirm_cancelled(cx);
                        let dialog_gone = app.pending_close_confirm.is_none();
                        let still_there = app.workspace.active_tab().tree().len() == before;
                        dialog_shown && not_closed && dialog_gone && still_there
                    })
                    .unwrap_or(false);
                check(esc_ok, "確認ダイアログ: 表示 + Esc キャンセル");

                // 73b. 確認ダイアログの表示 + Enter 承認
                let enter_ok = window
                    .update(cx, |app, _, cx| {
                        let before = app.workspace.active_tab().tree().len();
                        let target = app.focused_pane();
                        app.confirm_close = true;
                        app.close_pane_with_confirm(target, false, cx);
                        let dialog_shown = app.pending_close_confirm.is_some();
                        app.close_confirm_accepted(cx);
                        let after = app.workspace.active_tab().tree().len();
                        dialog_shown && after == before - 1
                    })
                    .unwrap_or(false);
                check(enter_ok, "確認ダイアログ: Enter で閉じる");

                // 73c. cmd+クリック = ダイアログスキップ
                type_text(
                    any,
                    cx,
                    &format!("{cli} split --right --focus >/dev/null"),
                    true,
                );
                wait(cx, 1500).await;
                let cmd_ok = window
                    .update(cx, |app, _, cx| {
                        let before = app.workspace.active_tab().tree().len();
                        let target = app.focused_pane();
                        app.confirm_close = true;
                        app.close_pane_with_confirm(target, true, cx);
                        let no_dialog = app.pending_close_confirm.is_none();
                        let after = app.workspace.active_tab().tree().len();
                        no_dialog && after == before - 1
                    })
                    .unwrap_or(false);
                check(cmd_ok, "確認ダイアログ: cmd+クリックでスキップ");

                // 75. ⌘K コマンドパレット（#217）: 開く → 絞り込み → Enter で実行 →
                //     テーマが反転し settings は汚さない（TAKO_SELF_TEST ガード）
                let palette_ok = window
                    .update(cx, |app, window, cx| {
                        app.open_command_palette(window, cx);
                        let opened = app.command_palette.is_some();
                        // 「テーマ」で絞ると toggle-theme コマンドが先頭に来る
                        if let Some(p) = app.command_palette.as_mut() {
                            p.query = "テーマ".into();
                            p.selected = 0;
                        }
                        let items = app.palette_items("テーマ");
                        let has_theme = matches!(
                            items.first(),
                            Some(PaletteItem::Command(_, "toggle-theme"))
                        );
                        let before_mode = app.theme.mode;
                        app.handle_palette_key(&Keystroke::parse("enter").unwrap(), cx);
                        let toggled =
                            app.theme.mode != before_mode && app.command_palette.is_none();
                        // 後始末: テーマを元に戻す
                        app.toggle_theme(cx);
                        let restored = app.theme.mode == before_mode;
                        // Esc で閉じる
                        app.open_command_palette(window, cx);
                        app.handle_palette_key(&Keystroke::parse("escape").unwrap(), cx);
                        let esc_closed = app.command_palette.is_none();
                        opened && has_theme && toggled && restored && esc_closed
                    })
                    .unwrap_or(false);
                check(palette_ok, "コマンドパレット: 絞り込み + Enter 実行 + Esc");

                // 73d. confirm_close=false で即クローズ
                type_text(
                    any,
                    cx,
                    &format!("{cli} split --right --focus >/dev/null"),
                    true,
                );
                wait(cx, 1500).await;
                let off_ok = window
                    .update(cx, |app, _, cx| {
                        let before = app.workspace.active_tab().tree().len();
                        let target = app.focused_pane();
                        app.confirm_close = false;
                        app.close_pane_with_confirm(target, false, cx);
                        let no_dialog = app.pending_close_confirm.is_none();
                        let after = app.workspace.active_tab().tree().len();
                        app.confirm_close = true; // 元に戻す
                        no_dialog && after == before - 1
                    })
                    .unwrap_or(false);
                check(off_ok, "確認ダイアログ: 設定 OFF で即クローズ");

                // 73e. タブの確認ダイアログ + キャンセル
                let tab_ok = window
                    .update(cx, |app, _, cx| {
                        let tab_id = app.workspace.active_tab_id();
                        app.confirm_close = true;
                        app.close_tab_with_confirm(tab_id, false, cx);
                        let dialog_shown = app.pending_close_confirm
                            == Some(CloseConfirmTarget::Tab(tab_id));
                        app.close_confirm_cancelled(cx);
                        let dialog_gone = app.pending_close_confirm.is_none();
                        dialog_shown && dialog_gone
                    })
                    .unwrap_or(false);
                check(tab_ok, "確認ダイアログ: タブの × でダイアログ表示 + キャンセル");
            }

            // 後片付け: 隔離した接続情報ディレクトリを消す
            if let Some(dir) = std::env::var_os("TAKO_DISCOVERY_DIR") {
                let _ = std::fs::remove_dir_all(dir);
            }

            // 最終項目: Quit はフォーカス喪失（blur）状態でも発火する（#103）。
            // 外部 a11y ツール等で window.blur() が起きると GPUI の dispatch path が
            // root dispatch node のみになり、ルート div の on_action ではキーバインド・
            // メニューどちらの経路でも Quit が不発だった。グローバル on_action 化後は
            // quit → on_app_quit フック（OK マーカー印字）→ 自然終了（exit 0）する。
            // 「終了すること」自体が成功条件のため必ず最後に置く。不発時のみ 5 秒後の
            // fail（exit 1・OK マーカーなし）へ到達する
            let _ = any.update(cx, |_, window, _| window.blur());
            press(any, cx, "cmd-q");
            wait(cx, 5000).await;
            fail("フォーカス喪失状態の cmd-q で終了しない (#103)");
        })
        .detach();
    }
}

/// .git ディレクトリが祖先に存在するかの軽量チェック（プロセス spawn なし。#313）
fn has_git_ancestor(dir: &std::path::Path) -> bool {
    let mut cur = dir;
    loop {
        if cur.join(".git").exists() {
            return true;
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return false,
        }
    }
}

fn find_git_root(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8(output.stdout).ok()?;
    let root = root.trim();
    if root.is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(root))
}

fn shell_escape(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if s.contains(|c: char| c.is_whitespace() || "\"'\\$`!#&|;(){}[]<>?*~".contains(c)) {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.into_owned()
    }
}

/// TAKO_ORCHESTRATOR_ROLE 環境変数の値（`master:<profile>` / `master` / `solo:<profile>` /
/// `worker:<project>` 等）からペインの role 文字列に変換する（#210）
fn role_from_orchestrator_env(env_role: &str) -> Option<String> {
    if let Some(suffix) = env_role.strip_prefix("master:") {
        if suffix.is_empty() {
            Some("orchestrator-master".into())
        } else {
            Some(format!("orchestrator-master:{suffix}"))
        }
    } else if env_role == "master" {
        Some("orchestrator-master".into())
    } else if let Some(suffix) = env_role.strip_prefix("solo:") {
        if suffix.is_empty() {
            Some("orchestrator-solo".into())
        } else {
            Some(format!("orchestrator-solo:{suffix}"))
        }
    } else if env_role == "solo" {
        Some("orchestrator-solo".into())
    } else if env_role.starts_with("worker:") {
        Some(format!("orchestrator-{env_role}"))
    } else {
        None
    }
}

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
mod guard_tests {
    use super::live_foreign_tako_ancestor;
    use std::collections::HashMap;

    // 検出の正常系（祖先に生きた tako-app が居る）は実プロセスが必要なため
    // 実機 e2e（隔離 HOME での before/after 実証）で担保する。ここでは
    // 否定系と防御（循環 ppid・自プロセス除外）を固定する

    #[test]
    fn 祖先に生きたtakoが無ければ検出しない() {
        // 100 → 50 → 1（launchd 相当）。どれも tako-app ではない
        let parents: HashMap<u32, u32> = [(100, 50), (50, 1)].into();
        assert_eq!(live_foreign_tako_ancestor(100, &parents), None);
    }

    #[test]
    fn ppidの循環でも無限ループしない() {
        let parents: HashMap<u32, u32> = [(100, 50), (50, 100)].into();
        assert_eq!(live_foreign_tako_ancestor(100, &parents), None);
    }

    #[test]
    fn 親情報が無いpidは検出しない() {
        assert_eq!(live_foreign_tako_ancestor(12345, &HashMap::new()), None);
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::{chunk_line_chars, link_byte_range_in_chunk, CharInfo};

    fn ci(ch: char, char_cols: usize, run_idx: usize, snaps: bool) -> CharInfo {
        CharInfo {
            ch,
            char_cols,
            run_idx,
            bg: None,
            snaps,
        }
    }

    fn text(infos: &[CharInfo], start: usize, end: usize) -> String {
        infos[start..end].iter().map(|x| x.ch).collect()
    }

    #[test]
    fn 同スタイルの半角は1チャンクにまとまる() {
        let infos: Vec<CharInfo> = "Fable 5 + max".chars().map(|c| ci(c, 1, 0, true)).collect();
        let chunks = chunk_line_chars(&infos);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].cols, 13);
        assert_eq!(
            text(&infos, chunks[0].start, chunks[0].end),
            "Fable 5 + max"
        );
    }

    #[test]
    fn 全角は単独チャンクで半角グループと分離される() {
        // 「ターミナルUI」: 全角 5 + 半角 2
        let mut infos: Vec<CharInfo> = "ターミナル".chars().map(|c| ci(c, 2, 0, false)).collect();
        infos.push(ci('U', 1, 0, true));
        infos.push(ci('I', 1, 0, true));
        let chunks = chunk_line_chars(&infos);
        assert_eq!(chunks.len(), 6); // 全角 5 個 + "UI" 1 グループ
        for c in &chunks[..5] {
            assert_eq!(c.end - c.start, 1);
            assert_eq!(c.cols, 2);
        }
        assert_eq!(text(&infos, chunks[5].start, chunks[5].end), "UI");
        assert_eq!(chunks[5].cols, 2);
    }

    #[test]
    fn セル幅不一致グリフは単独チャンクに隔離される() {
        // 「⏺ Fable」: ⏺ はフォールバックフォント（advance ≠ セル幅）想定
        let mut infos = vec![ci('⏺', 1, 0, false)];
        infos.extend(" Fable".chars().map(|c| ci(c, 1, 0, true)));
        let chunks = chunk_line_chars(&infos);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].end - chunks[0].start, 1); // ⏺ 単独
        assert_eq!(chunks[0].cols, 1);
        assert_eq!(text(&infos, chunks[1].start, chunks[1].end), " Fable");
    }

    #[test]
    fn スタイル境界で分割される() {
        let mut infos: Vec<CharInfo> = "ab".chars().map(|c| ci(c, 1, 0, true)).collect();
        infos.extend("cd".chars().map(|c| ci(c, 1, 1, true)));
        let chunks = chunk_line_chars(&infos);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].run_idx, 0);
        assert_eq!(chunks[1].run_idx, 1);
    }

    #[test]
    fn ゼロ幅文字は直前のグループに含まれる() {
        // 結合文字（char_cols == 0）はベース文字と同じ StyledText に居ないと合成されない
        let infos = vec![
            ci('a', 1, 0, true),
            ci('\u{0301}', 0, 0, false), // 結合アクセント
            ci('b', 1, 0, true),
        ];
        let chunks = chunk_line_chars(&infos);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].cols, 2);
        assert_eq!(chunks[0].end, 3);
    }

    #[test]
    fn リンク装飾は同一styleチャンク内のリンク範囲だけに限定する() {
        let text = "prefix https://example.com suffix";
        let infos: Vec<CharInfo> = text.chars().map(|c| ci(c, 1, 0, true)).collect();
        let cell_cols: Vec<usize> = (0..infos.len()).collect();
        let chunks = chunk_line_chars(&infos);
        assert_eq!(chunks.len(), 1, "同じ ANSI style なので描画チャンクは1つ");
        let start = text.find("https://").unwrap();
        let end = start + "https://example.com".len();
        let range = link_byte_range_in_chunk(&infos, &cell_cols, &chunks[0], start, end)
            .expect("リンク部分がチャンク内にある");
        assert_eq!(&text[range], "https://example.com");
    }
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
mod persist_resume_tests {
    use super::claude_resume_command;

    #[test]
    fn backend消失時だけ検証済みclaudeをresumeする() {
        let id = "a45899a8-96a6-4fa6-9bf6-71df53307878";
        assert_eq!(
            claude_resume_command(false, Some(id), true),
            Some(format!("claude --resume {id}\r").into_bytes())
        );
        // 通常の tako 再起動は既存プロセスへ再 attach し、Claude を二重起動しない
        assert_eq!(claude_resume_command(true, Some(id), true), None);
        // transcript 不在・不正 ID・ID 不明を推測で起動しない
        assert_eq!(claude_resume_command(false, Some(id), false), None);
        assert_eq!(claude_resume_command(false, Some("../../bad"), true), None);
        assert_eq!(claude_resume_command(false, None, true), None);
    }
}

#[cfg(test)]
mod role_env_tests {
    use super::role_from_orchestrator_env;

    #[test]
    fn master各形式からroleに変換する() {
        assert_eq!(
            role_from_orchestrator_env("master"),
            Some("orchestrator-master".into())
        );
        assert_eq!(
            role_from_orchestrator_env("master:exam"),
            Some("orchestrator-master:exam".into())
        );
        assert_eq!(
            role_from_orchestrator_env("master:"),
            Some("orchestrator-master".into())
        );
    }

    #[test]
    fn solo各形式からroleに変換する() {
        assert_eq!(
            role_from_orchestrator_env("solo"),
            Some("orchestrator-solo".into())
        );
        assert_eq!(
            role_from_orchestrator_env("solo:docs"),
            Some("orchestrator-solo:docs".into())
        );
    }

    #[test]
    fn worker形式からroleに変換する() {
        assert_eq!(
            role_from_orchestrator_env("worker:demo"),
            Some("orchestrator-worker:demo".into())
        );
    }

    #[test]
    fn 不明な形式は_none() {
        assert_eq!(role_from_orchestrator_env("unknown"), None);
        assert_eq!(role_from_orchestrator_env(""), None);
    }
}

#[cfg(test)]
mod git_ancestor_tests {
    use super::has_git_ancestor;

    #[test]
    fn gitリポジトリ内のディレクトリで_true() {
        let dir = std::env::current_dir().unwrap();
        assert!(has_git_ancestor(&dir));
    }

    #[test]
    fn gitリポジトリ内のサブディレクトリで_true() {
        let dir = std::env::current_dir().unwrap().join("crates");
        if dir.is_dir() {
            assert!(has_git_ancestor(&dir));
        }
    }

    #[test]
    fn ルートディレクトリで_false() {
        assert!(!has_git_ancestor(std::path::Path::new("/")));
    }

    #[test]
    fn 一時ディレクトリで_false() {
        let dir = std::env::temp_dir().join("tako-test-no-git");
        let _ = std::fs::create_dir_all(&dir);
        assert!(!has_git_ancestor(&dir));
        let _ = std::fs::remove_dir(&dir);
    }
}

#[cfg(test)]
mod menu_position_tests {
    use super::compute_menu_position;

    #[test]
    fn メニューが収まる場合はそのまま() {
        let (x, y) = compute_menu_position(100.0, 200.0, 180.0, 250.0, 1200.0, 800.0);
        assert_eq!(x, 100.0);
        assert_eq!(y, 200.0);
    }

    #[test]
    fn 右端を超えたら左へフリップ() {
        let (x, y) = compute_menu_position(1100.0, 200.0, 180.0, 250.0, 1200.0, 800.0);
        assert_eq!(x, 1100.0 - 180.0);
        assert_eq!(y, 200.0);
    }

    #[test]
    fn 下端を超えたら上へフリップ() {
        let (x, y) = compute_menu_position(100.0, 700.0, 180.0, 250.0, 1200.0, 800.0);
        assert_eq!(x, 100.0);
        assert_eq!(y, 700.0 - 250.0);
    }

    #[test]
    fn 両方超えたら両方フリップ() {
        let (x, y) = compute_menu_position(1100.0, 700.0, 180.0, 250.0, 1200.0, 800.0);
        assert_eq!(x, 1100.0 - 180.0);
        assert_eq!(y, 700.0 - 250.0);
    }

    #[test]
    fn フリップ後も負にならない() {
        let (x, y) = compute_menu_position(50.0, 30.0, 180.0, 250.0, 100.0, 100.0);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
    }
}

#[cfg(test)]
mod drag_scroll_tests {
    use super::*;

    #[test]
    fn 最小速度係数では最小速度になる() {
        let delta = drag_scroll_delta(0.0);
        let expected = DRAG_SCROLL_MIN_SPEED * (DRAG_SCROLL_INTERVAL_MS as f32 / 1000.0);
        assert!((delta - expected).abs() < 0.001);
    }

    #[test]
    fn 最大速度係数では最大速度になる() {
        let delta = drag_scroll_delta(1.0);
        let expected = DRAG_SCROLL_MAX_SPEED * (DRAG_SCROLL_INTERVAL_MS as f32 / 1000.0);
        assert!((delta - expected).abs() < 0.001);
    }

    #[test]
    fn 中間係数は最小と最大の間に収まる() {
        let min = drag_scroll_delta(0.0);
        let max = drag_scroll_delta(1.0);
        let mid = drag_scroll_delta(0.5);
        assert!(mid > min);
        assert!(mid < max);
    }

    #[test]
    fn 速度は単調増加する() {
        let mut prev = drag_scroll_delta(0.0);
        for i in 1..=10 {
            let factor = i as f32 / 10.0;
            let delta = drag_scroll_delta(factor);
            assert!(delta > prev, "factor={factor}: {delta} <= {prev}");
            prev = delta;
        }
    }
}

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
    /// tmuxview タブ（FR-2.13）を表示中か。タブモデルには入れない固定 UI 状態
    tmuxview_active: bool,
    /// tmux 一覧の最新スナップショット（dispatch の TmuxList 結果。表示は描画側の責務）
    tmux_sessions: Vec<serde_json::Value>,
    /// kill の確認待ち（セッション名, window index）。誤爆防止（FR-2.13.3）
    tmux_pending_kill: Option<(String, Option<u32>)>,
    /// タブ・ペイン名の AI 自動リネームの検知状態（FR-2.12。ループは new で張る）
    autorename: autorename::AutoRenamer,
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

        let mut app = Self {
            // ルートペインは下の spawn_session でセッションを張る
            workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
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
            tmuxview_active: false,
            tmux_sessions: Vec::new(),
            tmux_pending_kill: None,
            autorename: autorename::AutoRenamer::new(initial_auto_rename()),
        };
        let root_id = app.workspace.active_tab().tree().focused();
        if let Err(e) = app.spawn_session(root_id, SpawnOptions::default(), cx) {
            // 最初のペインすら開けない環境では使いようがない。SIGABRT ではなく明示終了する
            eprintln!("fatal: 最初のシェルを起動できない: {e}");
            std::process::exit(1);
        }

        // IPC リクエストを UI スレッドで dispatch するループ。
        // 操作セマンティクスは tako-control::dispatch に一元化されている（設計原則 5）
        cx.spawn(async move |this, cx| {
            while let Some(incoming) = control_rx.next().await {
                let result = this.update(cx, |app: &mut TakoApp, cx| {
                    let mut result = tako_control::dispatch(app, incoming.request, incoming.origin);
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

        // tmuxview 表示中は 2 秒毎に一覧を更新する（FR-2.13。表示していない間は何もしない）
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let result = this.update(cx, |app: &mut TakoApp, cx| {
                if app.tmuxview_active {
                    app.refresh_tmux(cx);
                }
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

    /// tmux 一覧を更新する。UI も CLI / MCP と同じコマンド層（dispatch）を通す
    fn refresh_tmux(&mut self, cx: &mut Context<Self>) {
        let value = tako_control::dispatch(
            self,
            tako_control::protocol::Request::TmuxList { socket: None },
            PaneOrigin::User,
        )
        .unwrap_or_else(|_| serde_json::json!({ "sessions": [] }));
        self.tmux_sessions = value["sessions"].as_array().cloned().unwrap_or_default();
        cx.notify();
    }

    /// 確認済みの kill を実行する（kill ボタン → 確認 → ここ）
    fn tmux_kill_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some((session, window)) = self.tmux_pending_kill.take() else {
            return;
        };
        let result = tako_control::dispatch(
            self,
            tako_control::protocol::Request::TmuxKill {
                socket: None,
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
                }
            }
            Err(_) => {
                // LastTab: アプリ終了は UI 層の責務
                cx.quit();
            }
        }
        cx.notify();
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
        self.tmuxview_active = false;
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
        self.tmuxview_active = false;
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
        let disambiguate = self
            .focused_session()
            .map(|s| s.disambiguate_keys())
            .unwrap_or(false);
        if let Some(bytes) = keystroke_to_bytes(keystroke, disambiguate) {
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
    fn pane_cursor_origin(&self, pane: PaneId) -> Option<Point<Pixels>> {
        let (_, area) = self.pane_text_areas.iter().find(|(id, _)| *id == pane)?;
        let cell = self.cell_size?;
        let (col, row) = self.terminals.get(&pane)?.screen(&self.theme).cursor?;
        Some(point(
            area.origin.x + cell.width * col as f32,
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

    /// tmuxview タブの中身（FR-2.13。データは `tmux_sessions` = dispatch の TmuxList 結果）。
    /// 表示方法は変わる前提（FR-2.13.5）なので、ここは JSON を読んで並べるだけに留める
    fn render_tmuxview(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme.clone();
        let sessions = self.tmux_sessions.clone();
        let pending = self.tmux_pending_kill.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let mut root = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(rgba(theme.background))
            .text_color(hsla(theme.foreground))
            .text_size(px(13.0))
            .overflow_hidden()
            .child(
                div()
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .text_size(px(11.0))
                    .child("実行中の tmux セッション（2 秒毎に更新。kill は確認つき）"),
            );
        if sessions.is_empty() {
            return root.child(
                div()
                    .text_color(hsla(theme.tab_inactive_foreground))
                    .child("実行中の tmux セッションはない"),
            );
        }
        for (index, session) in sessions.iter().enumerate() {
            let name = session["name"].as_str().unwrap_or("?").to_string();
            let attached = session["attached"].as_bool().unwrap_or(false);
            let created = session["created"].as_i64().unwrap_or(0);
            // tako との対応付け: クライアントごとに tako のタブ・ペイン / tako 外を表示する
            let clients: Vec<String> = session["clients"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| match (c["tab"].as_u64(), c["pane"].as_u64()) {
                    (Some(tab), Some(pane)) => format!("tako タブ {tab} / ペイン {pane}"),
                    _ => format!("tako 外（{}）", c["tty"].as_str().unwrap_or("?")),
                })
                .collect();
            let location = if clients.is_empty() {
                "detached（どこにも表示されていない）".to_string()
            } else {
                clients.join("、")
            };
            let windows: Vec<(u32, String)> = session["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|w| {
                    let w_index = w["index"].as_u64().unwrap_or(0) as u32;
                    (
                        w_index,
                        format!(
                            "{}:{}（{} ペイン）",
                            w_index,
                            w["name"].as_str().unwrap_or("?"),
                            w["panes"].as_u64().unwrap_or(0),
                        ),
                    )
                })
                .collect();

            let kill_name = name.clone();
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
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .child(SharedString::from(name.clone())),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(if attached {
                                    hsla(theme.accent)
                                } else {
                                    hsla(theme.ansi[3]) // 黄: 消し忘れ候補
                                })
                                .child(if attached { "attached" } else { "detached" }),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.tab_inactive_foreground))
                                .child(SharedString::from(format!(
                                    "作成 {} ・ {}",
                                    format_age(now - created),
                                    location,
                                ))),
                        )
                        .child(div().flex_grow(1.0))
                        .child(
                            div()
                                .id(("tmux-kill", index as u64))
                                .px_2()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(11.0))
                                .text_color(hsla(theme.ansi[1]))
                                .hover(|d| d.bg(rgba_alpha(theme.ansi[1], 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tmux_pending_kill = Some((kill_name.clone(), None));
                                    cx.notify();
                                }))
                                .child("kill"),
                        ),
                )
                .children(windows.into_iter().map(|(w_index, label)| {
                    let kill_name = name.clone();
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .pl_4()
                        .text_size(px(12.0))
                        .child(SharedString::from(label))
                        .child(
                            div()
                                .id(("tmux-kill-window", ((index as u64) << 16) | w_index as u64))
                                .px_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_size(px(10.0))
                                .text_color(hsla_alpha(theme.ansi[1], 0.8))
                                .hover(|d| d.bg(rgba_alpha(theme.ansi[1], 0.2)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tmux_pending_kill =
                                        Some((kill_name.clone(), Some(w_index)));
                                    cx.notify();
                                }))
                                .child("kill"),
                        )
                }));
            // 誤爆防止のインライン確認（FR-2.13.3）
            if let Some((pending_session, pending_window)) = &pending {
                if *pending_session == name {
                    let label = match pending_window {
                        Some(w) => format!("window {w} を kill する？（中のプロセスごと終了）"),
                        None => {
                            format!("セッション {name} を kill する？（中のプロセスごと終了）")
                        }
                    };
                    card = card.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .pl_4()
                            .text_size(px(12.0))
                            .text_color(hsla(theme.ansi[1]))
                            .child(SharedString::from(label))
                            .child(
                                div()
                                    .id(("tmux-kill-yes", index as u64))
                                    .px_2()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .bg(rgba_alpha(theme.ansi[1], 0.25))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tmux_kill_confirmed(cx);
                                    }))
                                    .child("kill する"),
                            )
                            .child(
                                div()
                                    .id(("tmux-kill-no", index as u64))
                                    .px_2()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .bg(rgba_alpha(theme.tab_bar_background, 0.9))
                                    .text_color(hsla(theme.foreground))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tmux_pending_kill = None;
                                        cx.notify();
                                    }))
                                    .child("やめる"),
                            ),
                    );
                }
            }
            root = root.child(card);
        }
        root
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
        let Some(session) = self.terminals.get(&pane_id) else {
            return;
        };
        let history = session.history_size();
        let (_, rows) = session.size();
        let total = (history + rows) as f32;
        let ratio = ((f32::from(y) - f32::from(area.origin.y)) / f32::from(area.size.height))
            .clamp(0.0, 1.0);
        // 表示窓（rows 行）の中心をマウス位置の行へ合わせ、上端行 → offset に直す
        let top_row = (ratio * total - rows as f32 / 2.0).clamp(0.0, history as f32);
        session.scroll_to(history - top_row.round() as usize);
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
            {
                cx.notify();
            }
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
        if self.dragging_border.take().is_some() | self.dragging_scrollbar.take().is_some() {
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
        let lines = match event.delta {
            ScrollDelta::Lines(l) => (l.y * 3.0) as i32,
            ScrollDelta::Pixels(p) => (f32::from(p.y) / f32::from(cell.height)) as i32,
        };
        if lines != 0 {
            // mouse reporting / alternate scroll / 自前スクロールの出し分けはセッション側
            let (col, row) = self
                .cell_at(pane_id, event.position, window)
                .map(|(c, r, _)| (c, r))
                .unwrap_or((0, 0));
            if let Some(session) = self.terminals.get(&pane_id) {
                session.scroll_wheel(lines, col, row);
                cx.notify();
            }
        }
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
                        this.tmuxview_active = false;
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
            .child(
                // 右端固定の tmuxview タブ（FR-2.13.1。閉じる × は持たない）
                div()
                    .id("tab-tmuxview")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h_full()
                    .px_3()
                    .cursor_pointer()
                    .when(self.tmuxview_active, |d| {
                        d.bg(rgba(theme.tab_active_background))
                            .border_b_2()
                            .border_color(hsla(theme.accent))
                    })
                    .text_color(if self.tmuxview_active {
                        hsla(theme.tab_active_foreground)
                    } else {
                        hsla(theme.tab_inactive_foreground)
                    })
                    .text_size(px(12.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tmuxview_active = true;
                        this.refresh_tmux(cx);
                    }))
                    .child("tmux"),
            )
    }

    fn render_pane(
        &mut self,
        pane_id: PaneId,
        rect: Rect,
        area: Bounds<Pixels>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

        // role / title バッジ（FR-2.1.3）。`tako title` / MCP で設定されたときだけ右上に重ねる。
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
        let state_dot = self
            .terminals
            .get(&pane_id)
            .and_then(|s| match s.command_state() {
                tako_core::CommandState::Failed(_) => Some(theme.ansi[1]),
                tako_core::CommandState::Running => Some(theme.accent),
                _ => None,
            });

        // スクロールバー（FR-2.5.13 の UI）。通常画面で履歴があるときだけ控えめに重ねる。
        // alternate screen（TUI）はスクロールバックが無いので出さない
        let scrollbar = self.terminals.get(&pane_id).and_then(|s| {
            if s.is_alt_screen() {
                return None;
            }
            let history = s.history_size();
            if history == 0 {
                return None;
            }
            let (_, rows) = s.size();
            let total = (history + rows) as f32;
            let track_h = f32::from(area.size.height);
            let thumb_h = (rows as f32 / total * track_h).clamp(20.0, track_h);
            let top =
                ((history - s.display_offset()) as f32 / total * track_h).min(track_h - thumb_h);
            let dragging = self.dragging_scrollbar == Some(pane_id);
            Some((top, thumb_h, track_h, dragging))
        });

        let screen = self.terminals.get(&pane_id).map(|s| s.screen(&theme));

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
            .p(px(PANE_PADDING))
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
            .children(lines)
            .children(Some(
                // 閉じるボタン（iTerm2 風。左上に控えめなオーバーレイ）
                div()
                    .id(("pane-close", pane_id.as_u64()))
                    .absolute()
                    .top(px(2.0))
                    .left(px(4.0))
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .occlude()
                    .text_size(px(11.0))
                    .text_color(hsla_alpha(theme.tab_inactive_foreground, 0.5))
                    .hover(|d| {
                        d.bg(rgba_alpha(theme.tab_bar_background, 0.9))
                            .text_color(hsla(theme.foreground))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| {
                            // ペインの選択開始・フォーカス処理に流さない
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_pane_button(pane_id, cx);
                    }))
                    .child("×"),
            ))
            .children(scrollbar.map(|(top, thumb_h, track_h, dragging)| {
                div()
                    .id(("scrollbar", pane_id.as_u64()))
                    .absolute()
                    .top(px(0.0))
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
                                if dragging { 0.7 } else { 0.35 },
                            )),
                    )
            }))
            .children((badge_label.is_some() || state_dot.is_some()).then(|| {
                div()
                    .absolute()
                    .top(px(2.0))
                    .right(px(6.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .rounded_sm()
                    .bg(rgba(theme.tab_bar_background))
                    .text_size(px(10.0))
                    .text_color(hsla(theme.accent))
                    .children(
                        state_dot.map(|color| {
                            div().w(px(6.0)).h(px(6.0)).rounded_full().bg(hsla(color))
                        }),
                    )
                    .children(badge_label.map(|label| SharedString::from(truncate(&label, 32))))
            }))
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
        let origin = self.pane_cursor_origin(self.ime_target())?;
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

/// キー入力 → PTY バイト列。`disambiguate` は kitty keyboard protocol の
/// disambiguate フラグ（TUI が `CSI > 1 u` で有効化。Claude Code 等が
/// Shift+Enter を区別するために使う）。有効時は Esc と修飾付き
/// Enter / Tab / Backspace を CSI u 形式で送る。
/// それ以外のフラグ（REPORT_ALL_KEYS 等）は未対応（必要になったら拡張する）
fn keystroke_to_bytes(ks: &Keystroke, disambiguate: bool) -> Option<Vec<u8>> {
    let mods = encode_modifiers(&ks.modifiers);
    if disambiguate {
        let code: Option<u32> = match ks.key.as_str() {
            "escape" => Some(27),
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

        // アクティブタブのレイアウト（単位矩形）と、マウス変換用のピクセル矩形を更新する
        let viewport = window.viewport_size();
        let content_origin = point(px(0.0), px(TAB_BAR_HEIGHT));
        let content_size = size(viewport.width, viewport.height - px(TAB_BAR_HEIGHT));
        let tree = self.workspace.active_tab().tree();
        let focused = tree.focused();
        let layout = tree.layout(Rect::UNIT);
        self.pane_text_areas = layout
            .iter()
            .map(|(id, r)| {
                let inset = PANE_BORDER + PANE_PADDING;
                let origin = point(
                    content_origin.x + content_size.width * r.x + px(inset),
                    content_origin.y + content_size.height * r.y + px(inset),
                );
                let area_size = size(
                    content_size.width * r.width - px(inset * 2.0),
                    content_size.height * r.height - px(inset * 2.0),
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
            let anchor = self.pane_cursor_origin(ime.pane)?;
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
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
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
            .child(if self.tmuxview_active {
                self.render_tmuxview(cx)
            } else {
                div()
                    .flex_1()
                    .relative()
                    .children(panes)
                    .children(border_handles)
                    .children(ime_overlay)
            })
            .child(ime_registration)
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.bind_keys(key_bindings());
        let bounds = Bounds::centered(None, size(px(960.), px(600.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
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
            check(status == 200 && tool_count == 17, "MCP tools/list は 17 ツール");

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

            // 49. tmuxview タブの状態遷移（FR-2.13.1。表示 → 一覧更新 → タブ操作で復帰。
            //     一覧は既定サーバーの読み取りのみで無害。kill の確認フローも畳めること）
            let view_ok = window
                .update(cx, |app, _, cx| {
                    app.tmuxview_active = true;
                    app.refresh_tmux(cx);
                    let shown = app.tmuxview_active;
                    // 確認フロー: 存在しないセッションの kill は無害に失敗し pending が畳まれる
                    app.tmux_pending_kill = Some(("tako-no-such-session".into(), None));
                    app.tmux_kill_confirmed(cx);
                    let pending_cleared = app.tmux_pending_kill.is_none();
                    app.activate_tab_index(0, cx);
                    shown && pending_cleared && !app.tmuxview_active
                })
                .unwrap_or(false);
            check(view_ok, "tmuxview の表示・確認フロー・復帰");

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

            println!("TAKO_APP_SELF_TEST_OK");
            std::process::exit(0);
        })
        .detach();
    }
}

/// 特殊キー → PTY 送出バイト列の総点検（バイトレベル検証）。
/// 実 IME / GUI を起動できない CI でもキーエンコードの退行を捕まえる
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
            keystroke_to_bytes(&ks_shift("enter"), true),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_ctrl("enter"), true),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("tab"), true),
            Some(b"\x1b[9;2u".to_vec())
        );
        assert_eq!(
            keystroke_to_bytes(&ks_shift("backspace"), true),
            Some(b"\x1b[127;2u".to_vec())
        );
        // Esc は単押しでも CSI u（disambiguate の仕様）
        assert_eq!(
            keystroke_to_bytes(&ks("escape"), true),
            Some(b"\x1b[27u".to_vec())
        );
        // 無修飾 Enter / Tab / Backspace はレガシーのまま
        assert_eq!(keystroke_to_bytes(&ks("enter"), true), Some(b"\r".to_vec()));
        assert_eq!(keystroke_to_bytes(&ks("tab"), true), Some(b"\t".to_vec()));
        assert_eq!(
            keystroke_to_bytes(&ks("backspace"), true),
            Some(b"\x7f".to_vec())
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
        keystroke_to_bytes(ks, false)
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

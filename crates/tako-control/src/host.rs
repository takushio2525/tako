//! host — ControlHost の責務別サブトレイト定義（Issue #86）
//!
//! 74 メソッドに肥大した `ControlHost` を 8 つのサブトレイトへ分割し、
//! `ControlHost` はスーパートレイトとして合成する。dispatch の呼び出し
//! シグネチャ（`&mut dyn ControlHost`）は不変。

use serde_json::Value;
use tako_core::{
    PaneId, PreviewOutline, PreviewOutlineTarget, PreviewViewState, PreviewViewUpdate,
    SpawnOptions, TabId, TerminalSession, Workspace,
};

/// ピン留め中のプレビュー 1 件分（FR-2.16.15。list / MCP 公開用）。
/// `group=false` なら `id` はペイン ID、`group=true` なら閉じたタブグループの由来タブ ID
#[derive(Debug, Clone, PartialEq)]
pub struct PinnedView {
    pub group: bool,
    pub id: u64,
    pub x: f32,
    pub y: f32,
}

// ---------------------------------------------------------------------------
// WorkspaceHost — ワークスペースアクセス（2 メソッド）
// ---------------------------------------------------------------------------

pub trait WorkspaceHost {
    fn workspace(&self) -> &Workspace;
    fn workspace_mut(&mut self) -> &mut Workspace;
}

// ---------------------------------------------------------------------------
// SessionHost — セッション管理 + 送信キュー（10 メソッド）
// ---------------------------------------------------------------------------

pub trait SessionHost {
    /// ペインのターミナルセッション（send / read / list の画面情報に使う）
    fn session(&self, pane: PaneId) -> Option<&TerminalSession>;
    /// ツリーへ挿入済みの新ペインに対しセッションを起動しイベント中継を張る。
    /// `TAKO_PANE_ID` 等の環境変数合成は実装側の責務（FR-2.1.1）
    fn attach_session(&mut self, pane: PaneId, options: SpawnOptions);
    /// 閉じられたペインのセッションを破棄する
    fn detach_session(&mut self, pane: PaneId);
    /// バックグラウンドから復帰させたペインのセッションを再接続する（FR-2.15.3）。
    /// セッション自体はバックグラウンド送り時に破棄していないため、UI 層で再描画するだけでよい場合が多い
    fn reattach_backgrounded(&mut self, _pane: PaneId) {}
    /// セッション起動後にペインへ書き込む遅延キュー。`attach_session` が非同期（pending_attach）
    /// のため、dispatch 内で直接 `session.write()` しても session がまだ存在しない。
    /// この関数で登録すると、セッション起動後に自動的に書き込まれる
    fn queue_write(&mut self, _pane: PaneId, _data: Vec<u8>) {}
    /// alt_screen 遷移後にペインへ書き込む遅延キュー。claude TUI の起動完了（alt_screen 遷移）を
    /// 待ってからプロンプトを送信するために使う。タイムアウト（60 秒）で自動破棄
    fn queue_write_on_alt_screen(&mut self, _pane: PaneId, _data: Vec<u8>) {}
    /// claude TUI へのプロンプト送信フローを登録する。画面内容を確認しながら
    /// 信頼ダイアログ承諾 → ❯ 待ち → 貼り付け → 分離 Enter → 入力欄の空検証 + 再送
    /// のステートマシンを駆動する（Issue #32 送達確認ループ）
    fn queue_prompt_flow(&mut self, _pane: PaneId, _prompt: String) {}
    /// 送達確認つき送信フローを登録する（Issue #32）。`queue_prompt_flow` と同じ
    /// ステートマシンだが claude TUI の起動を待たず、現画面へ即座に貼り付ける
    /// （全画面 TUI への newline つき送信用）。既定実装は何もしない（テスト用モック等）
    fn queue_send_flow(&mut self, _pane: PaneId, _text: String) {}
    /// Enter 単独の送達確認フローを登録する（Issue #95）。入力欄に残留した
    /// テキストの送信代行: 貼り付けせず Enter を送り、入力欄が空へ戻るまで
    /// 単独再送する。既定実装は何もしない（テスト用モック等）
    fn queue_enter_flow(&mut self, _pane: PaneId) {}
}

// ---------------------------------------------------------------------------
// TmuxHost — tmux バックエンド永続化 + ビュー + スクロール（10 メソッド）
// ---------------------------------------------------------------------------

pub trait TmuxHost {
    /// tmux バックエンド永続化（Phase 5.5 / FR-5）の現在状態
    fn tmux_persist_enabled(&self) -> bool {
        false
    }
    /// tmux バックエンド永続化の ON/OFF 切替（永続化は実装側の責務。以後のペインに効く）
    fn set_tmux_persist(&mut self, _enabled: bool) {}
    /// ペインを保持している tmux バックエンドセッション名（tmuxview の区別表示用。
    /// バックエンドでないペイン・非対応実装では None）
    fn backend_session(&self, _pane: PaneId) -> Option<String> {
        None
    }
    /// ペインのスクロール実体が tmux 側にあるか（バックエンドセッション、または
    /// `tako tmux open` の TmuxOpen ビュー。#181）。dispatch の `Scroll` をミラー経路
    /// （`backend_scroll_view`）へ回すかの判定。UI を持たない実装の既定は
    /// バックエンドセッションの有無
    fn is_mirror_scroll_pane(&self, pane: PaneId) -> bool {
        self.backend_session(pane).is_some()
    }
    /// バックエンドペインの表示スクロール（ローカルミラー方式 #159）。
    /// `to` = 絶対位置（0 = 最下部）/ `delta` = 相対行数（正 = 遡る）のどちらか一方。
    /// 戻り値は (クランプ後の表示位置, 履歴行数)。UI を持たない実装では None
    /// （= バックエンドのスクロール表示は不可）
    fn backend_scroll_view(
        &mut self,
        _pane: PaneId,
        _to: Option<usize>,
        _delta: Option<i32>,
    ) -> Option<(usize, usize)> {
        None
    }
    /// バックエンドセッション内の window 一覧（2+ window の場合のみ）
    fn backend_windows(&self, _pane: PaneId) -> Option<Vec<tako_core::TmuxWindow>> {
        None
    }
    /// TmuxOpen ペインの監視対象を登録する。`session` は監視・再 attach 対象の
    /// **元セッション**（ラッパー名は入れない）。`wrapper` は表示用の `tako-view-*`
    /// grouped session 名で、ペイン close 時に kill する（`None` = 元セッションを直接
    /// attach したので close 時も kill しない）。元セッション消滅で自動クローズする
    fn track_tmux_view(
        &mut self,
        _pane: PaneId,
        _session: String,
        _wrapper: Option<String>,
        _socket: Option<String>,
    ) {
    }
    /// orphan tmux セッションの一括クリーンアップ（FR-2.16.11）。実装側が現存ペイン・
    /// バックグラウンドペイン・表示中ビューを protected として除外し、backend socket 上の取り残し
    /// セッションを kill する。kill した名前を返す
    fn cleanup_orphan_tmux(&self) -> Vec<String> {
        Vec::new()
    }
    /// サイドバー tmux ビューでタブ枠が折りたたまれているか（FR-2.16.14）
    fn tmux_tab_collapsed(&self, _tab: TabId) -> bool {
        false
    }
    /// タブ枠の折りたたみを設定する（FR-2.16.14）。`collapsed` 省略時はトグル。
    /// 永続化は実装側の責務
    fn set_tmux_tab_collapsed(&mut self, _tab: TabId, _collapsed: Option<bool>) {}
}

// ---------------------------------------------------------------------------
// UiStateHost — パネル / ファイルツリー / ピン留め / 設定トグル（14 メソッド）
// ---------------------------------------------------------------------------

pub trait UiStateHost {
    /// AI 自動リネーム（FR-2.12.4）の現在状態。UI 層が検知ループの状態を返す
    fn auto_rename_enabled(&self) -> bool {
        true
    }
    /// AI 自動リネームの ON/OFF 切替（永続化は実装側の責務）
    fn set_auto_rename(&mut self, _enabled: bool) {}
    /// listen ポート検知 + 提案チップ（FR-2.4.4）の現在状態
    fn port_detect_enabled(&self) -> bool {
        true
    }
    /// listen ポート検知の ON/OFF 切替（永続化・検知済み情報の掃除は実装側の責務）
    fn set_port_detect(&mut self, _enabled: bool) {}
    /// × ボタン close の確認ダイアログの現在状態（Issue #172）
    fn confirm_close_enabled(&self) -> bool {
        true
    }
    /// 確認ダイアログの ON/OFF 切替（永続化は実装側の責務）
    fn set_confirm_close(&mut self, _enabled: bool) {}
    /// 右サイドバー情報パネルの状態 (visible, width, view)
    fn panel_state(&self) -> (bool, f32, crate::protocol::PanelViewWire) {
        (false, 0.0, crate::protocol::PanelViewWire::Tmux)
    }
    /// 右サイドバー情報パネルの操作（None の項目は変更しない）
    fn set_panel(
        &mut self,
        _visible: Option<bool>,
        _width: Option<f32>,
        _view: Option<crate::protocol::PanelViewWire>,
    ) {
    }
    /// 左サイドバーのファイルツリー（FR-3.1）の表示状態（FR-2.16.5）
    fn filetree_visible(&self) -> bool {
        false
    }
    /// ファイルツリーの表示・非表示（root の cwd 同期は実装側の責務）
    fn set_filetree(&mut self, _visible: bool) {}
    /// ファイルツリーの root 同期をトリガーする（#134: pinned_folders 変更後に呼ぶ）
    fn sync_filetree(&mut self) {}
    /// ピン留め中のプレビュー一覧（FR-2.16.15）
    fn pinned_previews(&self) -> Vec<PinnedView> {
        Vec::new()
    }
    /// ペインのプレビューをピン留め / 解除する（FR-2.16.15）。`pinned` 省略時はトグル
    fn set_pin_pane(&mut self, _pane: PaneId, _pinned: Option<bool>) {}
    /// 閉じたタブグループのプレビューをピン留め / 解除する（FR-2.16.15 / FR-2.16.16）
    fn set_pin_group(&mut self, _tab: TabId, _pinned: Option<bool>) {}
    /// UI テーマモードの現在値（Issue #217。ライト/ダーク切替）
    fn theme_mode(&self) -> tako_core::theme::ThemeMode {
        tako_core::theme::ThemeMode::Dark
    }
    /// UI テーマモードの切替（再描画は実装側の責務。永続化は dispatch 側で行う）
    fn set_theme_mode(&mut self, _mode: tako_core::theme::ThemeMode) {}
}

// ---------------------------------------------------------------------------
// PreviewHost — プレビュー + 編集 + 動画（17 メソッド）
// ---------------------------------------------------------------------------

pub trait PreviewHost {
    /// ペインのプレビュー状態（FR-3.2。`(path, mode)`。プレビューペインでなければ None）
    fn preview_state(&self, _pane: PaneId) -> Option<(String, crate::protocol::PreviewModeWire)> {
        None
    }
    /// ペインをプレビューペインにする / 表示内容を差し替える（読み込みは実装側の責務）
    fn set_preview(
        &mut self,
        _pane: PaneId,
        _path: &str,
        _mode: crate::protocol::PreviewModeWire,
    ) -> Result<(), String> {
        Ok(())
    }
    /// PDF・画像プレビューの現在のズーム / パン / ページ状態。
    fn preview_view_state(&self, _pane: PaneId) -> Option<PreviewViewState> {
        None
    }
    /// core の表示更新を適用する。PDF の最大ページ検査と実スクロール反映は実装側の責務。
    fn update_preview_view(
        &mut self,
        _pane: PaneId,
        _update: PreviewViewUpdate,
    ) -> Result<PreviewViewState, String> {
        Err("PDF・画像プレビューのズームは未対応".into())
    }
    /// ロード時に構築済みの Markdown / PDF アウトライン。
    fn preview_outline(&self, _pane: PaneId) -> Option<PreviewOutline> {
        None
    }
    /// 1 始まりのアウトライン項目へジャンプする。
    fn navigate_preview_outline(
        &mut self,
        _pane: PaneId,
        _item: usize,
    ) -> Result<PreviewOutlineTarget, String> {
        Err("アウトラインナビゲーションは未対応".into())
    }
    /// 表示中ファイルのライブリロード設定（Issue #233）。
    fn preview_reload_enabled(&self) -> bool {
        true
    }
    /// ライブリロードの ON/OFF 切替。監視登録と永続化は実装側の責務。
    fn set_preview_reload(&mut self, _enabled: bool) {}
    /// プレビュー編集状態（editing, dirty）。編集セッション未開始なら (false, false)。
    fn preview_edit_state(&self, _pane: PaneId) -> Option<(bool, bool)> {
        None
    }
    /// 編集モード切替。開始時のファイル読み込み・UTF-8 検査は実装側が core API で行う。
    fn set_preview_editing(&mut self, _pane: PaneId, _enabled: bool) -> Result<(), String> {
        Err("プレビュー編集は未対応".into())
    }
    /// 編集バッファの全文置換。
    fn apply_preview_text(&mut self, _pane: PaneId, _text: String) -> Result<(), String> {
        Err("プレビュー編集は未対応".into())
    }
    /// 編集バッファを保存。外部変更検知を含む保存セマンティクスは core API が担う。
    fn save_preview(&mut self, _pane: PaneId) -> Result<(), String> {
        Err("プレビュー編集は未対応".into())
    }
    /// undo（#195）
    fn preview_undo(&mut self, _pane: PaneId) -> Result<bool, String> {
        Err("プレビュー編集は未対応".into())
    }
    /// redo（#195）
    fn preview_redo(&mut self, _pane: PaneId) -> Result<bool, String> {
        Err("プレビュー編集は未対応".into())
    }
    /// 自動保存の状態取得（#195）
    fn preview_autosave(&self, _pane: PaneId) -> Option<bool> {
        None
    }
    /// 自動保存の設定変更（#195）
    fn set_preview_autosave(&mut self, _pane: PaneId, _enabled: bool) -> Result<(), String> {
        Err("プレビュー編集は未対応".into())
    }
    /// 検索クエリの設定とヒット取得（#195）
    fn preview_search(
        &mut self,
        _pane: PaneId,
        _query: Option<String>,
        _direction: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        Err("プレビュー編集は未対応".into())
    }
    /// 置換（#195）
    fn preview_replace(
        &mut self,
        _pane: PaneId,
        _query: &str,
        _replacement: &str,
        _all: bool,
    ) -> Result<serde_json::Value, String> {
        Err("プレビュー編集は未対応".into())
    }
    /// タブ内の既存プレビューペイン（OpenFile の再利用先。VSCode のプレビュータブ相当）
    fn preview_pane_of_tab(&self, _tab: TabId) -> Option<PaneId> {
        None
    }
    /// 動画プレイヤーの操作（"play" / "pause" / "toggle" / "mute" / "unmute" /
    /// "toggle_mute" / "loop_on" / "loop_off" / "toggle_loop"）。
    /// 戻り値は現在の state（"playing" / "paused"）
    fn video_playback(&mut self, _pane: PaneId, _action: &str) -> Result<String, String> {
        Err("動画再生は未対応".into())
    }
    /// 動画のシーク（秒）。戻り値は実際のシーク先の秒数
    fn video_seek(&mut self, _pane: PaneId, _seconds: f64) -> Result<f64, String> {
        Err("動画再生は未対応".into())
    }
    /// 動画の音量設定（0.0〜1.0）。戻り値は設定後の音量
    fn video_volume(&mut self, _pane: PaneId, _volume: f64) -> Result<f64, String> {
        Err("動画再生は未対応".into())
    }
}

// ---------------------------------------------------------------------------
// WebViewHost — Web ビュー wry/WKWebView（9 メソッド）
// ---------------------------------------------------------------------------

pub trait WebViewHost {
    /// Web ビューを生成してペインへ表示する（FR-3.8 / #155）。UI 層で wry WebView を
    /// 生成する。失敗時は Err を返し、呼び出し元がペインを巻き戻す。
    /// 成功時は `{ "id": u64, "pane": u64, "url": String }` を返す
    fn web_open(&mut self, _pane: PaneId, _url: &str) -> Result<Value, String> {
        Err("Web ビューはこの環境では使えない".into())
    }
    /// dock 退避中の Web ビュー `id` をペインへ表示する（FR-3.8 / #155）
    fn web_show(&mut self, _pane: PaneId, _id: u64) -> Result<Value, String> {
        Err("Web ビューはこの環境では使えない".into())
    }
    /// Web ビューの一覧（表示中 + dock 退避中）
    fn web_list(&self) -> Value {
        serde_json::json!([])
    }
    /// Web ビュー操作の対象解決。`id` 優先 → `pane`（表示中のもの）→ 省略時は
    /// 表示中が 1 つだけならそれ。戻り値は (id, 表示中ペイン)
    fn web_target(
        &self,
        _id: Option<u64>,
        _pane: Option<u64>,
    ) -> Result<(u64, Option<PaneId>), String> {
        Err("Web ビューはこの環境では使えない".into())
    }
    /// Web ビュー `id` を完全に破棄する。表示中だった場合はそのペイン ID を返す
    /// （ペイン自体の close は呼び出し元 = dispatch の責務）
    fn web_destroy(&mut self, _id: u64) -> Option<PaneId> {
        None
    }
    /// ナビゲーション（`to` = "back" / "forward" / "reload" / URL）
    fn web_navigate(&mut self, _id: u64, _to: &str) -> Result<Value, String> {
        Err("Web ビューはこの環境では使えない".into())
    }
    /// JS の非同期評価を発行し token を返す（結果は `web_eval_result` で回収）
    fn web_eval(&mut self, _id: u64, _js: &str) -> Result<Value, String> {
        Err("Web ビューはこの環境では使えない".into())
    }
    /// eval 結果の回収。未完なら `{ "pending": true }`
    fn web_eval_result(&mut self, _id: u64, _token: u64) -> Result<Value, String> {
        Err("Web ビューはこの環境では使えない".into())
    }
    /// URL・タイトル・読み込み状態を返す
    fn web_read(&self, _id: u64) -> Result<Value, String> {
        Err("Web ビューはこの環境では使えない".into())
    }
}

// ---------------------------------------------------------------------------
// RemoteHost — リモートアクセス API（3 メソッド）
// ---------------------------------------------------------------------------

pub trait RemoteHost {
    /// リモートアクセス API サーバーを起動する。成功時は状態 JSON を返す。
    /// 既定は暗号化トンネル必須。`insecure` = true のときだけ平文 LAN 直モードを許可する（#104）
    fn remote_start(&mut self, _port: Option<u16>, _insecure: bool) -> Result<Value, String> {
        Err("リモートアクセス API はこの環境では使えない".into())
    }
    /// リモートアクセス API サーバーを停止する
    fn remote_stop(&mut self) -> Result<Value, String> {
        Err("リモートアクセス API サーバーが起動していない".into())
    }
    /// リモートアクセス API サーバーの状態を返す
    fn remote_status(&self) -> Value {
        serde_json::json!({ "running": false })
    }
}

// ---------------------------------------------------------------------------
// SystemHost — 更新 / 診断 / ペインログ（11 メソッド）
// ---------------------------------------------------------------------------

pub trait SystemHost {
    /// セカンダリモード（Issue #113: 多重起動の後発。復元・layout 書き込み・persist 切替が
    /// 無効化されている）か。診断用に `tako persist` / MCP の応答へ含める
    fn is_secondary(&self) -> bool {
        false
    }
    /// 起動時のレイアウト復元結果（人間可読の 1 行。Issue #30 の診断用）。
    /// 復元を試みていない実装（テストホスト等）では None
    fn persist_restore_report(&self) -> Option<String> {
        None
    }
    /// 起動時に orphan 自動復帰した tmux セッション数（Issue #191）
    fn recovered_sessions_count(&self) -> usize {
        0
    }
    /// orphan 復元で旧 pane ID から新 pane ID を解決する（#210）。
    /// 既存 claude CLI が旧 TAKO_PANE_ID で MCP を呼んだとき、caller_pane を新 ID に変換する
    fn resolve_stale_pane(&self, _stale: PaneId) -> Option<PaneId> {
        None
    }
    /// バックエンド tmux セッション名の事前予約（Issue #112）。
    /// attach は非同期（pending_attach）のため、dispatch 時点では `backend_session` が
    /// まだ無い。spawn 応答の `tmux_session` とカタログの pending 記録のために、
    /// GUI は spawn_session と同じ採番でここで先に確定させる（persist OFF / tmux 不在は None）
    fn reserve_backend_session(&mut self, _pane: PaneId) -> Option<String> {
        None
    }
    /// アプリ内更新の診断情報（Issue #36）。配布系統・バージョン・重複 CLI を JSON で返す
    fn update_status(&self) -> Value {
        serde_json::json!({
            "current_version": "unknown",
            "install_method": "zip",
            "duplicate_cli": [],
        })
    }
    /// 更新チェック（Issue #36）。最新版の有無を JSON で返す（ブロッキング不可のため
    /// 同期呼び出しは非推奨。CLI / MCP はこの既定実装を使う）
    fn update_check(&self) -> Value {
        serde_json::json!({ "available": false })
    }
    /// 更新の実行（Issue #36）。配布系統に応じて brew upgrade or zip 差し替えを行う。
    /// UI 層は更新完了後に自動再起動する（dispatch は再起動しない）
    fn update_apply(&mut self) -> Result<Value, String> {
        Err("この環境では更新を実行できない".into())
    }
    /// zip 強制更新（#50）。brew 失敗時のフォールバック。配布系統を問わず zip で更新する
    fn update_apply_zip(&mut self) -> Result<Value, String> {
        Err("この環境では更新を実行できない".into())
    }
    /// broken-brew の修復（#50）。`brew install --cask --force` で台帳を再締結する
    fn update_repair(&mut self) -> Result<Value, String> {
        Err("この環境では修復を実行できない".into())
    }
    /// ペインログの現在設定（Issue #112 B）。GUI はライブの PaneLogManager から返す
    fn pane_log_config(&self) -> tako_core::pane_log::PaneLogConfig {
        crate::settings::load().pane_log_config()
    }
    /// ペインログ設定の反映（`tako logs set`）。GUI はライブの PaneLogManager へ適用する
    fn apply_pane_log_config(&mut self, _config: tako_core::pane_log::PaneLogConfig) {}
    /// ライブペインの現行ログファイル（Issue #112 B。クローズ済みペインは
    /// `pane_log::latest_for_pane` のファイル名検索にフォールバックする）
    fn pane_log_file(&self, _pane: PaneId) -> Option<std::path::PathBuf> {
        None
    }
}

// ---------------------------------------------------------------------------
// ControlHost — 全サブトレイトを合成するスーパートレイト
// ---------------------------------------------------------------------------

/// dispatch がドメイン状態へ触るためのホスト。UI 層（tako-app）とテストが実装する。
/// 個別の責務は上記サブトレイトに定義されており、ControlHost はそれらの合成。
/// `dyn ControlHost` として dispatch に渡される（シグネチャ不変）
pub trait ControlHost:
    WorkspaceHost
    + SessionHost
    + TmuxHost
    + UiStateHost
    + PreviewHost
    + WebViewHost
    + RemoteHost
    + SystemHost
{
}

/// 全サブトレイトを実装した型は自動的に ControlHost になる
impl<
        T: WorkspaceHost
            + SessionHost
            + TmuxHost
            + UiStateHost
            + PreviewHost
            + WebViewHost
            + RemoteHost
            + SystemHost,
    > ControlHost for T
{
}

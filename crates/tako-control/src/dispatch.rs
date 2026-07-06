//! dispatch — プロトコルリクエストを tako-core ドメイン API へ写す一元ディスパッチャ
//!
//! 設計原則 5「AI フルコントロール」の実装基盤: UI（tako-app）の IPC 受け口と
//! 将来の MCP サーバー（Phase 3）が**同じ dispatch** を呼ぶことで、操作セマンティクスを
//! 一箇所に保つ。各操作は `PaneTree` / `Workspace` の API と 1:1 対応（FR-2.5）。
//!
//! GPUI に依存する処理（セッション起動時のイベント中継、再描画通知）は
//! [`ControlHost`] trait の向こう側（UI 層）に置く。

use serde_json::{json, Value};
use tako_core::{
    CommandState, Pane, PaneId, PaneNode, PaneOrigin, PaneTreeError, Rect, SpawnCommand,
    SpawnOptions, SplitAxis, SplitDirection, TabId, TerminalSession, Workspace,
};

use crate::protocol::{error_code, Direction, FileOpKind, PreviewModeWire, Request};

/// ピン留め中のプレビュー 1 件分（FR-2.16.15。list / MCP 公開用）。
/// `group=false` なら `id` はペイン ID、`group=true` なら閉じたタブグループの由来タブ ID
#[derive(Debug, Clone, PartialEq)]
pub struct PinnedView {
    pub group: bool,
    pub id: u64,
    pub x: f32,
    pub y: f32,
}

/// dispatch がドメイン状態へ触るためのホスト。UI 層（tako-app）とテストが実装する
pub trait ControlHost {
    fn workspace(&self) -> &Workspace;
    fn workspace_mut(&mut self) -> &mut Workspace;
    /// ペインのターミナルセッション（send / read / list の画面情報に使う）
    fn session(&self, pane: PaneId) -> Option<&TerminalSession>;
    /// ツリーへ挿入済みの新ペインに対しセッションを起動しイベント中継を張る。
    /// `TAKO_PANE_ID` 等の環境変数合成は実装側の責務（FR-2.1.1）
    fn attach_session(&mut self, pane: PaneId, options: SpawnOptions);
    /// 閉じられたペインのセッションを破棄する
    fn detach_session(&mut self, pane: PaneId);
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
    /// tmux バックエンド永続化（Phase 5.5 / FR-5）の現在状態
    fn tmux_persist_enabled(&self) -> bool {
        false
    }
    /// tmux バックエンド永続化の ON/OFF 切替（永続化は実装側の責務。以後のペインに効く）
    fn set_tmux_persist(&mut self, _enabled: bool) {}
    /// 起動時のレイアウト復元結果（人間可読の 1 行。Issue #30 の診断用）。
    /// 復元を試みていない実装（テストホスト等）では None
    fn persist_restore_report(&self) -> Option<String> {
        None
    }
    /// ペインを保持している tmux バックエンドセッション名（tmuxview の区別表示用。
    /// バックエンドでないペイン・非対応実装では None）
    fn backend_session(&self, _pane: PaneId) -> Option<String> {
        None
    }
    /// バックエンドセッション内の window 一覧（2+ window の場合のみ）
    fn backend_windows(&self, _pane: PaneId) -> Option<Vec<tako_core::TmuxWindow>> {
        None
    }
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
    /// バックグラウンドから復帰させたペインのセッションを再接続する（FR-2.15.3）。
    /// セッション自体はバックグラウンド送り時に破棄していないため、UI 層で再描画するだけでよい場合が多い
    fn reattach_backgrounded(&mut self, _pane: PaneId) {}
    /// ファイルツリーの表示・非表示（root の cwd 同期は実装側の責務）
    fn set_filetree(&mut self, _visible: bool) {}
    /// ペインのプレビュー状態（FR-3.2。`(path, mode)`。プレビューペインでなければ None）
    fn preview_state(&self, _pane: PaneId) -> Option<(String, crate::protocol::PreviewModeWire)> {
        None
    }
    /// ペインをプレビューペインにする / 表示内容を差し替える（読み込みは実装側の責務）
    fn set_preview(&mut self, _pane: PaneId, _path: &str, _mode: crate::protocol::PreviewModeWire) {
    }
    /// タブ内の既存プレビューペイン（OpenFile の再利用先。VSCode のプレビュータブ相当）
    fn preview_pane_of_tab(&self, _tab: TabId) -> Option<PaneId> {
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
    /// ピン留め中のプレビュー一覧（FR-2.16.15）
    fn pinned_previews(&self) -> Vec<PinnedView> {
        Vec::new()
    }
    /// ペインのプレビューをピン留め / 解除する（FR-2.16.15）。`pinned` 省略時はトグル
    fn set_pin_pane(&mut self, _pane: PaneId, _pinned: Option<bool>) {}
    /// 閉じたタブグループのプレビューをピン留め / 解除する（FR-2.16.15 / FR-2.16.16）
    fn set_pin_group(&mut self, _tab: TabId, _pinned: Option<bool>) {}
    /// 動画プレイヤーの操作（"play" / "pause" / "toggle"）。プレビューペインが Video
    /// モードの場合のみ有効。戻り値は現在の state（"playing" / "paused"）
    fn video_playback(&mut self, _pane: PaneId, _action: &str) -> Result<String, String> {
        Err("動画再生は未対応".into())
    }
    /// 動画のシーク（秒）。戻り値は実際のシーク先の秒数
    fn video_seek(&mut self, _pane: PaneId, _seconds: f64) -> Result<f64, String> {
        Err("動画再生は未対応".into())
    }
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
    /// リモートアクセス API サーバーを起動する。成功時は状態 JSON を返す。
    /// `no_tunnel` = true で cloudflared を起動しない（LAN のみ）
    fn remote_start(&mut self, _port: Option<u16>, _no_tunnel: bool) -> Result<Value, String> {
        Err("リモートアクセス API はこの環境では使えない".into())
    }
    /// リモートアクセス API サーバーを停止する
    fn remote_stop(&mut self) -> Result<Value, String> {
        Err("リモートアクセス API サーバーが起動していない".into())
    }
    /// リモートアクセス API サーバーの状態を返す
    fn remote_status(&self) -> Value {
        json!({ "running": false })
    }
    /// Chrome を CDP ミラー方式で開く（FR-3.8 PoC）。UI 層で WebViewState を生成する。
    /// 失敗時は Err を返し、呼び出し元がペインを巻き戻す
    fn open_chrome(&mut self, _pane: PaneId, _url: &str) -> Result<(), String> {
        Ok(())
    }
    /// アプリ内更新の診断情報（Issue #36）。配布系統・バージョン・重複 CLI を JSON で返す
    fn update_status(&self) -> Value {
        json!({
            "current_version": "unknown",
            "install_method": "zip",
            "duplicate_cli": [],
        })
    }
    /// 更新チェック（Issue #36）。最新版の有無を JSON で返す（ブロッキング不可のため
    /// 同期呼び出しは非推奨。CLI / MCP はこの既定実装を使う）
    fn update_check(&self) -> Value {
        json!({ "available": false })
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
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum DispatchError {
    #[error("ペイン {0} が見つからない")]
    PaneNotFound(u64),
    #[error("タブ {0} が見つからない")]
    TabNotFound(u64),
    #[error("対象ペインが未指定（--pane 指定か TAKO_PANE_ID が必要）")]
    NoTargetPane,
    #[error("ペイン {0} にはターミナルセッションがない")]
    NoSession(u64),
    #[error("無効なパラメータ: {0}")]
    InvalidParams(String),
    #[error("{0}")]
    Operation(String),
}

impl DispatchError {
    /// JSON-RPC エラーコードへの対応付け
    pub fn code(&self) -> i64 {
        match self {
            DispatchError::InvalidParams(_) => error_code::INVALID_PARAMS,
            _ => error_code::OPERATION,
        }
    }
}

/// リクエストを実行し、成功時の `result` 値を返す。
/// `origin` は新規生成ペインの生成主体（Layer 1 CLI なら `Cli`、Phase 3 の MCP なら `Mcp`）
pub fn dispatch(
    host: &mut dyn ControlHost,
    request: Request,
    origin: PaneOrigin,
) -> Result<Value, DispatchError> {
    match request {
        Request::Split {
            pane,
            tab,
            direction,
            ratio,
            command,
            cwd,
            focus,
        } => {
            // tab 指定時はそのタブのフォーカス中ペインを基準にする（active tab 非依存）
            let (tab, target) = if let Some(tab_raw) = tab {
                let tab_id = find_tab(host.workspace(), tab_raw)?;
                let focused = host
                    .workspace()
                    .get_tab(tab_id)
                    .expect("find_tab で存在確認済み")
                    .tree()
                    .focused();
                (tab_id, focused)
            } else {
                resolve_pane(host.workspace(), pane)?
            };
            let new_pane = Pane::new(origin);
            let new_id = new_pane.id();
            // 呼び出し元（target）と同じタブに生える（FR-2.1.2）
            tree_mut(host.workspace_mut(), tab)
                .split_with_ratio(
                    target,
                    direction.unwrap_or(Direction::Right).to_core(),
                    ratio.unwrap_or(0.5),
                    new_pane,
                )
                .map_err(op_err)?;
            let options = SpawnOptions {
                command: command.filter(|c| !c.is_empty()).map(|mut c| SpawnCommand {
                    program: c.remove(0),
                    args: c,
                }),
                // cwd 未指定なら分割元ペインの cwd（OSC 7 通知）を継承する。
                // ssh 先などローカルに存在しないパスは無視しホーム既定に任せる
                cwd: cwd.map(Into::into).or_else(|| {
                    host.session(target)
                        .and_then(|s| s.cwd())
                        .filter(|p| p.is_dir())
                        .map(|p| p.to_path_buf())
                }),
                env: Vec::new(),
            };
            host.attach_session(new_id, options);
            // MCP/CLI 経由のデフォルトはフォーカスを移さない（ユーザーの入力を奪わない）
            if !focus.unwrap_or(false) {
                let _ = tree_mut(host.workspace_mut(), tab).focus(target);
            }
            Ok(json!({ "pane": new_id.as_u64() }))
        }

        Request::Close { pane, force } => {
            let (tab, target) = resolve_pane(host.workspace(), pane)?;

            // worker 保護: orchestrator-worker role のペインが busy なら拒否
            if !force {
                let is_worker = host
                    .workspace()
                    .get_tab(tab)
                    .and_then(|t| t.tree().get(target))
                    .and_then(|p| p.role())
                    .is_some_and(|r| r.starts_with("orchestrator-worker"));
                if is_worker {
                    let busy = is_worker_busy(host, target);
                    if busy {
                        return Err(DispatchError::Operation(format!(
                            "Worker is still active. Use force: true to close anyway. pane_id={}",
                            target.as_u64()
                        )));
                    }
                }
            }

            let closed = tree_mut(host.workspace_mut(), tab).close(target);
            match closed {
                Ok(_) => {}
                Err(PaneTreeError::LastPane) => {
                    // タブ最後の 1 ペイン → タブごと閉じる。最後のタブなら拒否する
                    // （アプリ終了に等しい操作は AI / CLI からは行わせない。UI の cmd+W のみ）
                    host.workspace_mut().close_tab(tab).map_err(op_err)?;
                }
                Err(e) => return Err(op_err(e)),
            }
            host.detach_session(target);
            Ok(json!({ "closed": target.as_u64() }))
        }

        Request::Focus { pane, direction } => {
            if let Some(direction) = direction {
                // 方向指定はアクティブタブ内の隣接移動（FR-2.5.5）
                let moved = host
                    .workspace_mut()
                    .active_tab_mut()
                    .tree_mut()
                    .focus_direction(direction.to_core());
                Ok(json!({ "focused": moved.map(|id| id.as_u64()) }))
            } else {
                let (tab, target) = resolve_pane(host.workspace(), pane)?;
                let ws = host.workspace_mut();
                tree_mut(ws, tab).focus(target).map_err(op_err)?;
                // 別タブのペインへのフォーカスはタブ切替も伴う
                ws.activate_tab(tab).map_err(op_err)?;
                Ok(json!({ "focused": target.as_u64() }))
            }
        }

        Request::Resize {
            pane,
            axis,
            delta,
            share,
        } => {
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
            let tree = tree_mut(host.workspace_mut(), tab);
            let new_share = match (delta, share) {
                (Some(d), None) => tree.resize_by(target, axis.to_core(), d).map_err(op_err)?,
                (None, Some(s)) => tree.set_share(target, axis.to_core(), s).map_err(op_err)?,
                _ => {
                    return Err(DispatchError::InvalidParams(
                        "delta か share のどちらか一方を指定する".into(),
                    ))
                }
            };
            Ok(json!({ "share": new_share }))
        }

        Request::Equalize { pane, tab } => {
            let tab_id = match tab {
                Some(raw) => find_tab(host.workspace(), raw)?,
                None => resolve_pane(host.workspace(), pane)?.0,
            };
            tree_mut(host.workspace_mut(), tab_id).equalize();
            Ok(Value::Null)
        }

        Request::List => Ok(list_json(host)),

        Request::Send {
            pane,
            text,
            newline,
            tmux_session,
            await_prompt,
        } => {
            // await_prompt: claude TUI の起動（❯ 表示）を待ってから送達確認つきで送信する。
            // pane が解決できず tmux_session がある場合はバックグラウンドの tmux 経路で同等を行う
            if await_prompt {
                return match resolve_pane(host.workspace(), pane) {
                    Ok((_, target)) => {
                        host.queue_prompt_flow(target, text.clone());
                        Ok(json!({ "queued": true }))
                    }
                    Err(e) => match tmux_session {
                        Some(ref ts) => {
                            spawn_tmux_delivery(ts.clone(), text.clone(), true);
                            Ok(json!({ "queued": true }))
                        }
                        None => Err(e),
                    },
                };
            }

            // pane ID で解決を試み、失敗時に tmux session フォールバック
            match resolve_pane(host.workspace(), pane) {
                Ok((_, target)) => {
                    let session = host
                        .session(target)
                        .ok_or(DispatchError::NoSession(target.as_u64()))?;
                    // 全画面 TUI（claude 等）への改行つき送信は送達確認フローへ（Issue #32:
                    // 一括書き込みは改行が「送信」と解釈されず入力欄に残留する）。
                    // シェルへの送信は従来どおり即時書き込み（挙動・レイテンシ据え置き）
                    if newline && session.is_alt_screen() {
                        host.queue_send_flow(target, text.clone());
                        return Ok(json!({ "queued": true }));
                    }
                    let payload = if newline {
                        format!("{text}\r")
                    } else {
                        text.clone()
                    };
                    session.write(payload.into_bytes());
                    Ok(Value::Null)
                }
                Err(e) => {
                    if let Some(ref ts) = tmux_session {
                        if newline {
                            // 改行つき送信は送達確認つき配送（対象が claude TUI なら
                            // 貼り付け + 分離 Enter + 検証、シェルなら即時に無害劣化）
                            spawn_tmux_delivery(ts.clone(), text.clone(), false);
                            Ok(json!({ "queued": true }))
                        } else {
                            let socket = tako_core::tmux_backend::socket_name();
                            tako_core::tmux::send_keys(Some(&socket), ts, &text)
                                .map_err(DispatchError::Operation)?;
                            Ok(Value::Null)
                        }
                    } else {
                        Err(e)
                    }
                }
            }
        }

        Request::Read {
            pane,
            lines,
            tmux_session,
        } => {
            // pane ID で解決を試み、失敗時に tmux session フォールバック
            let read_result = resolve_pane(host.workspace(), pane)
                .ok()
                .and_then(|(_, target)| {
                    host.session(target)
                        .map(|session| (target.as_u64(), session.visible_lines()))
                });

            let (pane_id, mut all) = match read_result {
                Some(r) => r,
                None => {
                    if let Some(ref ts) = tmux_session {
                        let socket = tako_core::tmux_backend::socket_name();
                        let captured = tako_core::tmux::capture_session(Some(&socket), ts)
                            .map_err(DispatchError::Operation)?;
                        (pane.unwrap_or(0), captured)
                    } else {
                        let (_, target) = resolve_pane(host.workspace(), pane)?;
                        return Err(DispatchError::NoSession(target.as_u64()));
                    }
                }
            };

            while all.last().is_some_and(|l| l.is_empty()) {
                all.pop();
            }
            if let Some(n) = lines {
                if all.len() > n {
                    all.drain(..all.len() - n);
                }
            }
            Ok(json!({ "pane": pane_id, "text": all.join("\n") }))
        }

        Request::Scroll { pane, to, delta } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let session = host
                .session(target)
                .ok_or(DispatchError::NoSession(target.as_u64()))?;
            if matches!((to, delta), (Some(_), Some(_)) | (None, None)) {
                return Err(DispatchError::InvalidParams(
                    "to（絶対位置。0 = 最下部）か delta（相対行数）のどちらか一方を指定する".into(),
                ));
            }
            // バックエンドペイン（Phase 5.5）のスクロールバックは tmux 側にある。
            // ネスト tmux（ペイン内 attach）まで含めて tako-core::scroll が解決・駆動する
            // （UI のホイール / スクロールバーと同じ層。開発不変条件）
            if let Some(backend) = host.backend_session(target) {
                let socket = tako_core::tmux_backend::socket_name();
                let scroll_target = tako_core::scroll::resolve_target(&socket, &backend, &[None]);
                let state = match (to, delta) {
                    (Some(offset), None) => {
                        tako_core::scroll::scroll_to(&scroll_target, offset as usize)
                    }
                    (None, Some(lines)) => tako_core::scroll::scroll_by(&scroll_target, lines),
                    _ => unreachable!("引数は上で検証済み"),
                }
                .ok_or_else(|| {
                    DispatchError::Operation("バックエンドセッションのスクロールに失敗".into())
                })?;
                return Ok(json!({
                    "pane": target.as_u64(),
                    "offset": state.position,
                    "history": state.history,
                }));
            }
            match (to, delta) {
                (Some(offset), None) => session.scroll_to(offset as usize),
                (None, Some(lines)) => session.scroll_display(lines),
                _ => unreachable!("引数は上で検証済み"),
            }
            Ok(json!({
                "pane": target.as_u64(),
                "offset": session.display_offset(),
                "history": session.history_size(),
            }))
        }

        Request::Title { pane, title, role } => {
            if title.is_none() && role.is_none() {
                return Err(DispatchError::InvalidParams(
                    "title か role の少なくとも一方を指定する".into(),
                ));
            }
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
            let pane = tree_mut(host.workspace_mut(), tab)
                .get_mut(target)
                .expect("resolve_pane で存在確認済み");
            if let Some(t) = title {
                pane.set_title((!t.is_empty()).then_some(t));
            }
            if let Some(r) = role {
                pane.set_role((!r.is_empty()).then_some(r));
            }
            Ok(Value::Null)
        }

        Request::TmuxList { socket } => {
            // tako ペインとの対応付け: attach クライアントの tty とペインの tty を
            // 突き合わせる（FR-2.13.2。一致しないクライアントは tako 外 = 外部ターミナル由来。
            // tmux バックエンドのペインは tty がバックエンド側ペイン tty に差し替わっており、
            // その中でユーザーが開いたネスト tmux のクライアントもこの突き合わせで対応付く）
            let ws = host.workspace();
            let pane_of_tty: Vec<(String, u64, u64)> = ws
                .tabs()
                .iter()
                .flat_map(|tab| {
                    tab.tree().panes().into_iter().filter_map(|p| {
                        let tty = host.session(p.id())?.tty_name()?;
                        Some((tty.to_string(), p.id().as_u64(), tab.id().as_u64()))
                    })
                })
                .collect();
            // tako 自身のバックエンドセッション（Phase 5.5）の対応表: セッション名 → ペイン
            let backend_of: Vec<(String, u64, u64)> = ws
                .tabs()
                .iter()
                .flat_map(|tab| {
                    tab.tree().panes().into_iter().filter_map(|p| {
                        let name = host.backend_session(p.id())?;
                        Some((name, p.id().as_u64(), tab.id().as_u64()))
                    })
                })
                .collect();
            let session_json = |s: &tako_core::TmuxSession, backend: bool, socket: &Value| {
                let clients: Vec<Value> = s
                    .client_ttys
                    .iter()
                    .map(|tty| {
                        let hit = pane_of_tty.iter().find(|(t, _, _)| t == tty);
                        json!({
                            "tty": tty,
                            // tako のどのペインで表示中か（null = tako 外のターミナル）
                            "pane": hit.map(|(_, pane, _)| *pane),
                            "tab": hit.map(|(_, _, tab)| *tab),
                        })
                    })
                    .collect();
                let owner = backend_of.iter().find(|(name, _, _)| *name == s.name);
                json!({
                    "name": s.name,
                    "created": s.created,
                    "attached": s.attached,
                    // tako のバックエンド永続化セッションか（FR-5。kill すると
                    // 対応ペインの中身が消えるため、UI / AI は区別して扱うこと）
                    "backend": backend,
                    "socket": socket,
                    // backend セッションを保持している tako ペイン（orphan なら null）
                    "backend_pane": owner.map(|(_, pane, _)| *pane),
                    "backend_tab": owner.map(|(_, _, tab)| *tab),
                    "windows": s.windows.iter().map(|w| json!({
                        "index": w.index,
                        "name": w.name,
                        "active": w.active,
                        "panes": w.panes,
                    })).collect::<Vec<_>>(),
                    "clients": clients,
                })
            };
            let backend_socket = tako_core::tmux_backend::socket_name();
            let explicit_backend = socket.as_deref() == Some(backend_socket.as_str());
            let mut sessions: Vec<Value> = tako_core::tmux::list_sessions(socket.as_deref())
                .iter()
                .map(|s| {
                    session_json(
                        s,
                        explicit_backend,
                        &socket.as_deref().map(Into::into).unwrap_or(Value::Null),
                    )
                })
                .collect();
            // 既定サーバーの一覧には tako バックエンドのセッションも併記する
            // （消し忘れの発見が FR-2.13 の目的。バックエンドの orphan も見えるべき）
            if socket.is_none() {
                sessions.extend(
                    tako_core::tmux::list_sessions(Some(&backend_socket))
                        .iter()
                        .map(|s| session_json(s, true, &backend_socket.clone().into())),
                );
            }
            Ok(json!({ "sessions": sessions }))
        }

        Request::TmuxKill {
            socket,
            session,
            window,
        } => {
            match window {
                Some(index) => tako_core::tmux::kill_window(socket.as_deref(), &session, index),
                None => tako_core::tmux::kill_session(socket.as_deref(), &session),
            }
            .map_err(DispatchError::Operation)?;
            Ok(json!({ "killed": session, "window": window }))
        }

        Request::TmuxResize {
            socket,
            session,
            window,
            cols,
            rows,
            reset,
        } => {
            if reset {
                tako_core::tmux::reset_window_size(socket.as_deref(), &session, window)
                    .map_err(DispatchError::Operation)?;
                return Ok(json!({ "session": session, "window": window, "reset": true }));
            }
            let (Some(cols), Some(rows)) = (cols, rows) else {
                return Err(DispatchError::InvalidParams(
                    "cols と rows の両方を指定するか、reset を使うこと".into(),
                ));
            };
            tako_core::tmux::resize_window(socket.as_deref(), &session, window, cols, rows)
                .map_err(DispatchError::Operation)?;
            Ok(json!({
                "session": session,
                "window": window,
                "cols": cols,
                "rows": rows,
            }))
        }

        Request::TmuxOpen {
            socket,
            session,
            window,
            pane,
            direction,
        } => {
            // 存在しないセッション名は分割前に弾く（D&D 経路では起こらないが、
            // CLI / MCP からのタイポで空ペインだけが生えるのを防ぐ）。
            // has-session（1 コマンド）で確認（旧 list_sessions は 3 コマンドで重かった）
            if !tako_core::tmux::has_session(socket.as_deref(), &session) {
                return Err(DispatchError::Operation(format!(
                    "tmux セッション {session} が見つからない（socket: {}）",
                    socket.as_deref().unwrap_or("既定")
                )));
            }
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
            let new_pane = Pane::new(origin);
            let new_id = new_pane.id();
            tree_mut(host.workspace_mut(), tab)
                .split_with_ratio(
                    target,
                    direction.unwrap_or(Direction::Right).to_core(),
                    0.5,
                    new_pane,
                )
                .map_err(op_err)?;
            // MCP/CLI 経由ではフォーカスを分割元に維持（ユーザーの入力を奪わない）
            let _ = tree_mut(host.workspace_mut(), tab).focus(target);
            // 元セッションの解決（無限ネスト防止 = 今回の根治）。tmux はグループ名を
            // 「最初に作られた元セッション名」にするため、`tako-view-*` ラッパーや grouped
            // session を開こうとしても group を辿れば必ず元へ戻る。
            // 例: `tako-view-tako-view-master-tako-2-0`（group=master-tako）→ `master-tako`
            let group = tako_core::tmux::session_group(socket.as_deref(), &session);
            let original = group.unwrap_or_else(|| session.clone());
            // tako 自身が作ったラッパーを開き直す場合（バックグラウンドからの復帰・再オープン等）は、
            // **新しいラッパーを作らず元セッションをそのまま直接 attach** する（ユーザー指示）。
            // この経路で開いたペインは元セッションそのものなので close 時に kill しない
            let reopen = session.starts_with("tako-view-");
            // `TMUX=` はネストガードの回避（tako バックエンドペイン内からでも実行可）
            let mut command = vec!["env".to_string(), "TMUX=".to_string(), "tmux".to_string()];
            if let Some(socket) = &socket {
                command.push("-L".into());
                command.push(socket.clone());
            }
            let wrapper = if reopen {
                // 復帰/再オープン: 元セッションを直接 attach（ラッパーを作らない）。
                // window 選択は元セッション全体に効く（独立ラッパーが無いため）
                command.extend([
                    "attach-session".to_string(),
                    "-t".to_string(),
                    format!("={original}"),
                ]);
                if let Some(w) = window {
                    command.extend([
                        ";".to_string(),
                        "select-window".to_string(),
                        "-t".to_string(),
                        format!("{w}"),
                    ]);
                }
                None
            } else {
                // 新規取り込み: grouped session で独立表示する（FR-2.16.10）。
                // `new-session -t <original>` は同じ window 群を共有しつつ表示 window は
                // 独立なので、元クライアント（親）の表示を巻き込まない。ラッパー名はペイン
                // ID で一意化し、同一セッションを複数ペインで開いても衝突しない。元では
                // なくこの **ラッパー** を close 時に kill する（元セッションは無傷）
                let name = format!("tako-view-{original}-{}", new_id.as_u64());
                command.extend([
                    "new-session".to_string(),
                    "-t".to_string(),
                    format!("={original}"),
                    "-s".to_string(),
                    name.clone(),
                ]);
                if let Some(w) = window {
                    // new-session -t では window 指定不可。作成後に select-window を ; で繋ぐ
                    command.extend([
                        ";".to_string(),
                        "select-window".to_string(),
                        "-t".to_string(),
                        format!("{w}"),
                    ]);
                }
                // クライアント切断時の自動破棄（残骸防止の保険。明示 kill が主経路）
                command.extend([
                    ";".to_string(),
                    "set".to_string(),
                    "destroy-unattached".to_string(),
                    "on".to_string(),
                ]);
                Some(name)
            };
            host.track_tmux_view(new_id, original.clone(), wrapper.clone(), socket.clone());
            let mut command = command.into_iter();
            host.attach_session(
                new_id,
                SpawnOptions {
                    command: Some(SpawnCommand {
                        program: command.next().expect("env が先頭にある"),
                        args: command.collect(),
                    }),
                    cwd: None,
                    env: Vec::new(),
                },
            );
            Ok(json!({
                "pane": new_id.as_u64(),
                // 解決後の元セッション名（ラッパー名を渡されても元へ正規化して返す）
                "session": original,
                // 表示用ラッパー名（直接 attach した復帰経路では null）
                "wrapper": wrapper,
                "socket": socket,
            }))
        }

        Request::TmuxSelectWindow { pane, window } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let session = host
                .backend_session(target)
                .ok_or_else(|| DispatchError::Operation(format!(
                    "ペイン {target} にバックエンドセッションがない（tmux 永続化が無効 or 直接 spawn）"
                )))?;
            let socket = tako_core::tmux_backend::socket_name();
            tako_core::tmux::select_window(Some(&socket), &session, window)
                .map_err(DispatchError::Operation)?;
            Ok(json!({
                "pane": target.as_u64(),
                "session": session,
                "window": window,
            }))
        }

        Request::TmuxCleanup { socket } => {
            // socket 省略時は tako バックエンドサーバーを対象にする（取り残しの主因）
            let _ = socket; // 現状は backend socket 固定（host が protected を解決して実行）
            let killed = host.cleanup_orphan_tmux();
            Ok(json!({ "killed": killed }))
        }

        Request::TabRename { pane, tab, title } => {
            let tab_id = match tab {
                Some(raw) => find_tab(host.workspace(), raw)?,
                None => resolve_pane(host.workspace(), pane)?.0,
            };
            let tab = host
                .workspace_mut()
                .get_tab_mut(tab_id)
                .expect("find_tab / resolve_pane で存在確認済み");
            if title.is_empty() {
                // 空文字 = 手動指定の解除（タイトルは保持し、自動リネームを再開させる）
                tab.clear_manual_title();
            } else {
                tab.set_title_manual(title);
            }
            Ok(json!({ "tab": tab_id.as_u64(), "title": tab.title() }))
        }

        Request::TabNew { title } => {
            let pane = Pane::new(origin);
            let pane_id = pane.id();
            let explicit = title.is_some();
            let title = title.unwrap_or_else(|| (host.workspace().tabs().len() + 1).to_string());
            let tab_id = host.workspace_mut().create_tab(title, pane);
            if explicit {
                // 明示タイトル付きの作成は手動リネーム扱い（自動リネームに上書きさせない）
                if let Some(tab) = host.workspace_mut().get_tab_mut(tab_id) {
                    let title = tab.title().to_string();
                    tab.set_title_manual(title);
                }
            }
            host.attach_session(pane_id, SpawnOptions::default());
            Ok(json!({ "tab": tab_id.as_u64(), "pane": pane_id.as_u64() }))
        }

        Request::TabSelect { tab } => {
            let tab_id = find_tab(host.workspace(), tab)?;
            host.workspace_mut().activate_tab(tab_id).map_err(op_err)?;
            Ok(Value::Null)
        }

        Request::MovePane {
            pane,
            tab,
            target,
            direction,
        } => {
            let (_, source) = resolve_pane(host.workspace(), pane)?;
            match (tab, target) {
                // 従来動作: 別タブの末尾（フォーカス右）へ移送
                (Some(tab), None) => {
                    if direction.is_some() {
                        return Err(DispatchError::InvalidParams(
                            "direction は target 指定時のみ使える".into(),
                        ));
                    }
                    let dest = find_tab(host.workspace(), tab)?;
                    host.workspace_mut()
                        .move_pane(source, dest)
                        .map_err(op_err)?;
                }
                // FR-1.10: target ペインの隣（direction 側）へ挿し直す
                (None, Some(raw)) => {
                    let (_, target) = resolve_pane(host.workspace(), Some(raw))?;
                    host.workspace_mut()
                        .move_pane_to(
                            source,
                            target,
                            direction.unwrap_or(Direction::Right).to_core(),
                        )
                        .map_err(op_err)?;
                }
                _ => {
                    return Err(DispatchError::InvalidParams(
                        "tab か target のどちらか一方を指定する".into(),
                    ))
                }
            }
            Ok(Value::Null)
        }

        Request::AutoRename { enabled } => {
            if let Some(enabled) = enabled {
                host.set_auto_rename(enabled);
            }
            Ok(json!({ "enabled": host.auto_rename_enabled() }))
        }

        Request::PortDetect { enabled } => {
            if let Some(enabled) = enabled {
                host.set_port_detect(enabled);
            }
            Ok(json!({ "enabled": host.port_detect_enabled() }))
        }

        Request::Persist { enabled } => {
            if let Some(enabled) = enabled {
                host.set_tmux_persist(enabled);
            }
            Ok(json!({
                "enabled": host.tmux_persist_enabled(),
                // tmux 不在環境では PTY が直接 spawn へ劣化していることを示す
                // （その場合もタブ構成の保存・復元は機能する。復元は新シェル）
                "available": tako_core::tmux_backend::available(),
                // 診断（Issue #30）: 保存先の実パスと存在有無・起動時の復元結果・ログ
                "layout_path": crate::layout::layout_path()
                    .map(|p| p.display().to_string()),
                "layout_exists": crate::layout::layout_path()
                    .map(|p| p.is_file())
                    .unwrap_or(false),
                "last_restore": host.persist_restore_report(),
                "log_path": crate::diag::persist_log_path()
                    .map(|p| p.display().to_string()),
            }))
        }

        Request::Panel {
            visible,
            width,
            view,
            filetree,
        } => {
            if let Some(w) = width {
                if !w.is_finite() || w <= 0.0 {
                    return Err(DispatchError::InvalidParams(
                        "width は正の数（px）を指定する".into(),
                    ));
                }
            }
            host.set_panel(visible, width, view);
            if let Some(filetree) = filetree {
                host.set_filetree(filetree);
            }
            let (visible, width, view) = host.panel_state();
            Ok(json!({
                "visible": visible,
                "width": width,
                "view": view.as_str(),
                "filetree": host.filetree_visible(),
            }))
        }

        Request::OpenFile {
            pane,
            path,
            mode,
            direction,
        } => {
            let (tab, target) = match pane {
                Some(_) => resolve_pane(host.workspace(), pane)?,
                None => {
                    let ws = host.workspace();
                    let active = ws.active_tab_id();
                    let focused = ws.active_tab().tree().focused();
                    (active, focused)
                }
            };
            // 相対パスは対象ペインの cwd（OSC 7。無ければプロセスの cwd）基準で解決する
            let mut resolved = std::path::PathBuf::from(&path);
            if resolved.is_relative() {
                if let Some(cwd) = host.session(target).and_then(|s| s.cwd()) {
                    resolved = cwd.join(resolved);
                }
            }
            let resolved = resolved.canonicalize().map_err(|e| {
                DispatchError::Operation(format!("ファイルを開けない（{path}: {e}）"))
            })?;
            if !resolved.is_file() {
                return Err(DispatchError::Operation(format!(
                    "ファイルではない: {}",
                    resolved.display()
                )));
            }
            let mode =
                mode.unwrap_or_else(|| match resolved.extension().and_then(|e| e.to_str()) {
                    Some(ext) if ext.eq_ignore_ascii_case("md") => PreviewModeWire::Markdown,
                    Some(ext) if ext.eq_ignore_ascii_case("markdown") => PreviewModeWire::Markdown,
                    Some(ext)
                        if matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"
                        ) =>
                    {
                        PreviewModeWire::Image
                    }
                    Some(ext) if ext.eq_ignore_ascii_case("pdf") => PreviewModeWire::Pdf,
                    Some(ext)
                        if matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "mp4" | "webm" | "mov" | "avi" | "mkv"
                        ) =>
                    {
                        PreviewModeWire::Video
                    }
                    _ => PreviewModeWire::Code,
                });
            // 表示先の解決: direction 指定（FR-3.11 = D&D のドロップ位置）なら再利用せず
            // 必ずその方向へ分割。省略時は 対象自身がプレビュー > 同タブの既存プレビュー
            // （再利用）> 右分割で新設（ターミナルセッションは起動しない = attach なし）
            let (view_pane, created) = if let Some(direction) = direction {
                let new_pane = Pane::new(origin);
                let new_id = new_pane.id();
                tree_mut(host.workspace_mut(), tab)
                    .split_with_ratio(target, direction.to_core(), 0.5, new_pane)
                    .map_err(op_err)?;
                (new_id, true)
            } else if host.preview_state(target).is_some() {
                (target, false)
            } else if let Some(existing) = host.preview_pane_of_tab(tab) {
                (existing, false)
            } else {
                let new_pane = Pane::new(origin);
                let new_id = new_pane.id();
                tree_mut(host.workspace_mut(), tab)
                    .split_with_ratio(target, SplitDirection::Right, 0.5, new_pane)
                    .map_err(op_err)?;
                (new_id, true)
            };
            let path_str = resolved.display().to_string();
            host.set_preview(view_pane, &path_str, mode);
            // 開いたものへフォーカスを移す（タブ切替はしない。見せる導線は Focus / FR-2.7）
            tree_mut(host.workspace_mut(), tab)
                .focus(view_pane)
                .map_err(op_err)?;
            Ok(json!({
                "pane": view_pane.as_u64(),
                "path": path_str,
                "mode": mode.as_str(),
                "created": created,
            }))
        }
        Request::FileOp {
            op,
            path,
            name,
            pane,
        } => {
            let path = std::path::PathBuf::from(&path);
            match op {
                FileOpKind::CopyAbsolutePath => {
                    let abs = if path.is_absolute() {
                        path
                    } else {
                        std::env::current_dir().unwrap_or_default().join(&path)
                    };
                    Ok(json!({ "path": abs.display().to_string() }))
                }
                FileOpKind::CopyRelativePath => {
                    let abs = if path.is_absolute() {
                        path.clone()
                    } else {
                        std::env::current_dir().unwrap_or_default().join(&path)
                    };
                    let rel = if let Some(pane_id) = pane {
                        let (_, target) = resolve_pane(host.workspace(), Some(pane_id))?;
                        host.session(target)
                            .and_then(|s| s.cwd())
                            .and_then(|cwd| pathdiff::diff_paths(&abs, cwd))
                            .unwrap_or_else(|| abs.clone())
                    } else {
                        abs.clone()
                    };
                    Ok(json!({ "path": rel.display().to_string() }))
                }
                FileOpKind::Reveal => {
                    if !path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "パスが存在しない: {}",
                            path.display()
                        )));
                    }
                    #[cfg(target_os = "macos")]
                    {
                        std::process::Command::new("open")
                            .arg("-R")
                            .arg(&path)
                            .spawn()
                            .map_err(|e| {
                                DispatchError::Operation(format!("Finder を開けない: {e}"))
                            })?;
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        return Err(DispatchError::Operation(
                            "Finder で表示は macOS のみ対応".into(),
                        ));
                    }
                    Ok(json!({ "revealed": path.display().to_string() }))
                }
                FileOpKind::OpenTerminal => {
                    let dir = dir_of(&path);
                    let (_, target) = resolve_pane(host.workspace(), pane)?;
                    host.session(target)
                        .ok_or(DispatchError::NoSession(target.as_u64()))?;
                    let cd_text = format!("cd {}\r", shell_escape(&dir.display().to_string()));
                    if let Some(session) = host.session(target) {
                        session.write(cd_text.as_bytes().to_vec());
                    }
                    Ok(json!({ "pane": target.as_u64(), "cwd": dir.display().to_string() }))
                }
                FileOpKind::Rename => {
                    let new_name =
                        name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
                    validate_name(&new_name)?;
                    let parent = path.parent().ok_or(DispatchError::Operation(
                        "親ディレクトリが取得できない".into(),
                    ))?;
                    let new_path = parent.join(&new_name);
                    if new_path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "既に存在する: {}",
                            new_path.display()
                        )));
                    }
                    std::fs::rename(&path, &new_path)
                        .map_err(|e| DispatchError::Operation(format!("リネームに失敗: {e}")))?;
                    Ok(
                        json!({ "old": path.display().to_string(), "new": new_path.display().to_string() }),
                    )
                }
                FileOpKind::CreateFile => {
                    let file_name =
                        name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
                    validate_name(&file_name)?;
                    let new_path = dir_of(&path).join(&file_name);
                    if new_path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "既に存在する: {}",
                            new_path.display()
                        )));
                    }
                    std::fs::File::create(&new_path).map_err(|e| {
                        DispatchError::Operation(format!("ファイル作成に失敗: {e}"))
                    })?;
                    Ok(json!({ "created": new_path.display().to_string() }))
                }
                FileOpKind::CreateDir => {
                    let dir_name =
                        name.ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
                    validate_name(&dir_name)?;
                    let new_path = dir_of(&path).join(&dir_name);
                    if new_path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "既に存在する: {}",
                            new_path.display()
                        )));
                    }
                    std::fs::create_dir(&new_path).map_err(|e| {
                        DispatchError::Operation(format!("フォルダ作成に失敗: {e}"))
                    })?;
                    Ok(json!({ "created": new_path.display().to_string() }))
                }
                FileOpKind::Trash => {
                    if !path.exists() {
                        return Err(DispatchError::Operation(format!(
                            "パスが存在しない: {}",
                            path.display()
                        )));
                    }
                    #[cfg(target_os = "macos")]
                    {
                        let posix = path.display().to_string();
                        // 制御文字（0x00-0x1F, 0x7F）を含むパスは AppleScript インジェクション防止のため拒否
                        if posix.bytes().any(|b| b <= 0x1F || b == 0x7F) {
                            return Err(DispatchError::Operation(
                                "パスに制御文字が含まれているため操作できません".to_string(),
                            ));
                        }
                        let script = format!(
                            "tell application \"Finder\" to delete (POSIX file \"{}\" as alias)",
                            posix.replace('\\', "\\\\").replace('"', "\\\"")
                        );
                        let out = std::process::Command::new("osascript")
                            .arg("-e")
                            .arg(&script)
                            .output()
                            .map_err(|e| {
                                DispatchError::Operation(format!("ゴミ箱への移動に失敗: {e}"))
                            })?;
                        if !out.status.success() {
                            let msg = String::from_utf8_lossy(&out.stderr);
                            return Err(DispatchError::Operation(format!(
                                "ゴミ箱への移動に失敗: {msg}"
                            )));
                        }
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        std::fs::remove_file(&path)
                            .or_else(|_| std::fs::remove_dir_all(&path))
                            .map_err(|e| DispatchError::Operation(format!("削除に失敗: {e}")))?;
                    }
                    Ok(json!({ "trashed": path.display().to_string() }))
                }
            }
        }
        Request::GitLog { pane, max_count } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let cwd = host
                .session(target)
                .and_then(|s| s.cwd())
                .ok_or(DispatchError::Operation("cwd が取得できない".into()))?;
            let repo = tako_core::git::repo_root(cwd)
                .ok_or(DispatchError::Operation("git リポジトリではない".into()))?;
            let max = max_count.unwrap_or(200);
            let commits = tako_core::git::log_commits(&repo, max);
            let branches = tako_core::git::list_branches(&repo);
            let status = tako_core::git::status(&repo);
            Ok(json!({
                "repo": repo.display().to_string(),
                "branch": status.branch,
                "upstream": status.upstream,
                "commits": commits.iter().map(|c| json!({
                    "hash": c.hash,
                    "short_hash": c.short_hash,
                    "author": c.author,
                    "date": c.date_relative,
                    "subject": c.subject,
                    "refs": c.refs,
                    "parents": c.parents,
                })).collect::<Vec<_>>(),
                "branches": branches.iter().map(|b| json!({
                    "name": b.name,
                    "current": b.is_current,
                    "remote": b.is_remote,
                    "hash": b.commit_hash,
                    "subject": b.subject,
                })).collect::<Vec<_>>(),
                "status": status.entries.iter().map(|e| json!({
                    "path": e.path,
                    "index": e.index.to_string(),
                    "worktree": e.worktree.to_string(),
                })).collect::<Vec<_>>(),
            }))
        }
        Request::GitDiff { pane, target } => {
            let (_, pane_id) = resolve_pane(host.workspace(), pane)?;
            let cwd = host
                .session(pane_id)
                .and_then(|s| s.cwd())
                .ok_or(DispatchError::Operation("cwd が取得できない".into()))?;
            let repo = tako_core::git::repo_root(cwd)
                .ok_or(DispatchError::Operation("git リポジトリではない".into()))?;
            let diff_target = match target.as_deref() {
                None | Some("unstaged") => tako_core::git::DiffTarget::Unstaged,
                Some("staged") => tako_core::git::DiffTarget::Staged,
                Some(hash) => tako_core::git::DiffTarget::Commit(hash.to_string()),
            };
            let files = tako_core::git::diff(&repo, &diff_target);
            Ok(json!({
                "repo": repo.display().to_string(),
                "files": files.iter().map(|f| json!({
                    "path": f.path,
                    "hunks": f.hunks.iter().map(|h| json!({
                        "header": h.header,
                        "lines": h.lines.iter().map(|l| json!({
                            "kind": match l.kind {
                                tako_core::DiffLineKind::Context => "context",
                                tako_core::DiffLineKind::Add => "add",
                                tako_core::DiffLineKind::Remove => "remove",
                            },
                            "content": l.content,
                        })).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }))
        }

        Request::Background { pane } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            host.workspace_mut().shelve_pane(target).map_err(op_err)?;
            Ok(json!({ "backgrounded": target.as_u64() }))
        }

        Request::Foreground {
            pane,
            target,
            direction,
        } => {
            let pane_id = PaneId::from_raw(pane);
            if !host.workspace().is_shelved(pane_id) {
                return Err(DispatchError::PaneNotFound(pane));
            }
            let target_id = if let Some(t) = target {
                let (_, id) = resolve_pane(host.workspace(), Some(t))?;
                id
            } else {
                let ws = host.workspace();
                ws.shelved_origin_tab(pane_id)
                    .and_then(|tab| ws.get_tab(tab))
                    .map(|tab| tab.tree().focused())
                    .unwrap_or_else(|| ws.active_tab().tree().focused())
            };
            let dir = direction
                .map(|d| d.to_core())
                .unwrap_or(SplitDirection::Right);
            host.workspace_mut()
                .unshelve_pane(pane_id, target_id, dir)
                .map_err(op_err)?;
            host.reattach_backgrounded(pane_id);
            Ok(json!({ "foregrounded": pane, "target": target_id.as_u64() }))
        }

        Request::BackgroundList => {
            let items: Vec<serde_json::Value> = host
                .workspace()
                .shelved_panes()
                .iter()
                .map(|p| {
                    let state = host
                        .session(p.id())
                        .map(|s| s.command_state())
                        .unwrap_or(CommandState::Unknown);
                    let cwd = host
                        .session(p.id())
                        .and_then(|s| s.cwd())
                        .map(|p| p.display().to_string());
                    json!({
                        "pane": p.id().as_u64(),
                        "title": p.title(),
                        "role": p.role(),
                        "state": format!("{state:?}").to_lowercase(),
                        "cwd": cwd,
                        "origin_tab": p.origin_tab().as_u64(),
                        "origin_tab_title": p.origin_tab_title(),
                        "surface": "background",
                    })
                })
                .collect();
            Ok(json!({ "backgrounded": items }))
        }

        Request::CollapseTab {
            pane,
            tab,
            collapsed,
        } => {
            let tab_id = match tab {
                Some(t) => find_tab(host.workspace(), t)?,
                None => resolve_pane(host.workspace(), pane)?.0,
            };
            host.set_tmux_tab_collapsed(tab_id, collapsed);
            Ok(json!({
                "tab": tab_id.as_u64(),
                "collapsed": host.tmux_tab_collapsed(tab_id),
            }))
        }

        Request::Pin {
            pane,
            group_tab,
            pinned,
        } => {
            if let Some(t) = group_tab {
                // 閉じたタブグループ: tab は閉じているので tabs() に無い。バックグラウンドペインの由来で検証
                let tab = TabId::from_raw(t);
                if !host
                    .workspace()
                    .shelved_panes()
                    .iter()
                    .any(|p| p.origin_tab() == tab)
                {
                    return Err(DispatchError::TabNotFound(t));
                }
                host.set_pin_group(tab, pinned);
                Ok(json!({ "pinned": pinned_json(host), "group_tab": t }))
            } else {
                let (_, target) = resolve_pane(host.workspace(), pane)?;
                host.set_pin_pane(target, pinned);
                Ok(json!({ "pinned": pinned_json(host), "pane": target.as_u64() }))
            }
        }

        Request::BackgroundKill { pane } => {
            let pane_id = PaneId::from_raw(pane);
            if host.workspace_mut().remove_shelved(pane_id).is_none() {
                return Err(DispatchError::PaneNotFound(pane));
            }
            host.detach_session(pane_id);
            Ok(json!({ "killed": pane }))
        }

        Request::CheckHealth => Ok(check_health(host)),

        Request::SetupMcp { scope, pane } => {
            let scope_str = scope.as_deref().unwrap_or("global");
            let settings_dir = match scope_str {
                "project" => {
                    let (_, target) = resolve_pane(host.workspace(), pane)?;
                    let cwd = host
                        .session(target)
                        .and_then(|s| s.cwd())
                        .ok_or(DispatchError::Operation("cwd が取得できない".into()))?;
                    cwd.join(".claude")
                }
                _ => home_dir()
                    .ok_or(DispatchError::Operation(
                        "ホームディレクトリが取得できない".into(),
                    ))?
                    .join(".claude"),
            };
            let tako_bin = resolve_tako_binary();
            let result = setup_mcp_settings(&tako_bin, &settings_dir.join("settings.json"))?;
            Ok(json!({
                "configured": result.configured,
                "already_existed": result.already_existed,
                "settings_path": settings_dir.join("settings.json").display().to_string(),
                "command": tako_bin,
            }))
        }

        Request::VideoPlayback { pane, action } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            if host.preview_state(target).map(|(_, m)| m) != Some(PreviewModeWire::Video) {
                return Err(DispatchError::Operation(
                    "対象ペインは動画プレビューではない".into(),
                ));
            }
            let state = host
                .video_playback(target, &action)
                .map_err(DispatchError::Operation)?;
            Ok(json!({ "pane": target.as_u64(), "state": state }))
        }

        Request::VideoSeek { pane, seconds } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            if host.preview_state(target).map(|(_, m)| m) != Some(PreviewModeWire::Video) {
                return Err(DispatchError::Operation(
                    "対象ペインは動画プレビューではない".into(),
                ));
            }
            let actual = host
                .video_seek(target, seconds)
                .map_err(DispatchError::Operation)?;
            Ok(json!({ "pane": target.as_u64(), "seconds": actual }))
        }

        Request::OrchestratorProjects {
            action,
            key,
            cwd,
            description,
        } => dispatch_orchestrator_projects(&action, key, cwd, description),

        Request::OrchestratorProfiles {
            action,
            name,
            model,
            worker_model,
            effort,
            worker_effort,
            clear_model,
            clear_worker_model,
        } => dispatch_orchestrator_profiles(ProfilesParams {
            action,
            name,
            model,
            worker_model,
            effort,
            worker_effort,
            clear_model,
            clear_worker_model,
        }),

        Request::OrchestratorSpawn {
            project,
            prompt,
            label,
            model,
            effort,
            pane,
            tab,
        } => dispatch_orchestrator_spawn(
            host,
            origin,
            &project,
            &prompt,
            label.as_deref(),
            model.as_deref(),
            effort.as_deref(),
            pane,
            tab,
        ),

        Request::OrchestratorWorkerStatus {
            pane_id,
            session_id,
            tmux_session,
        } => dispatch_orchestrator_worker_status(
            host,
            pane_id,
            session_id.as_deref(),
            tmux_session.as_deref(),
        ),

        Request::RemoteStart { port, no_tunnel } => host
            .remote_start(port, no_tunnel)
            .map_err(DispatchError::Operation),
        Request::RemoteStop => host.remote_stop().map_err(DispatchError::Operation),
        Request::RemoteStatus => Ok(host.remote_status()),

        // エージェント一覧と会話ログはどのプロセスでも取得できる（ControlHost 不要）
        Request::RemoteAgents => {
            crate::agents::list_agents_with_panes(None).map_err(DispatchError::Operation)
        }

        Request::RemoteMessages { session_id, tail } => {
            crate::transcript::read_messages(&session_id, tail.unwrap_or(30))
                .map_err(DispatchError::Operation)
        }

        Request::RemoteScrollback { pane_id, lines } => {
            let result = crate::remote::scrollback(&pane_id, lines.unwrap_or(1000))
                .map_err(DispatchError::Operation)?;
            Ok(json!({ "lines": result }))
        }

        Request::ChromeOpen {
            url,
            pane,
            direction,
        } => {
            let (tab, target) = match pane {
                Some(_) => resolve_pane(host.workspace(), pane)?,
                None => {
                    let ws = host.workspace();
                    let active = ws.active_tab_id();
                    let focused = ws.active_tab().tree().focused();
                    (active, focused)
                }
            };
            let dir = direction
                .map(|d| d.to_core())
                .unwrap_or(SplitDirection::Right);
            let new_pane = Pane::new(origin);
            let new_id = new_pane.id();
            tree_mut(host.workspace_mut(), tab)
                .split_with_ratio(target, dir, 0.5, new_pane)
                .map_err(op_err)?;
            if let Err(e) = host.open_chrome(new_id, &url) {
                let _ = tree_mut(host.workspace_mut(), tab).close(new_id);
                return Err(DispatchError::Operation(format!("Chrome 起動失敗: {e}")));
            }
            tree_mut(host.workspace_mut(), tab)
                .focus(new_id)
                .map_err(op_err)?;
            Ok(json!({ "pane": new_id.as_u64(), "url": url }))
        }

        Request::Update { action } => {
            let action = action.as_deref().unwrap_or("status");
            match action {
                "status" => Ok(host.update_status()),
                "check" => Ok(host.update_check()),
                "apply" => host.update_apply().map_err(DispatchError::Operation),
                "apply-zip" => host.update_apply_zip().map_err(DispatchError::Operation),
                "repair" => host.update_repair().map_err(DispatchError::Operation),
                other => Err(DispatchError::InvalidParams(format!(
                    "不明な action: {other:?}（status / check / apply / apply-zip / repair のいずれか）"
                ))),
            }
        }
    }
}

// --- オーケストレーター dispatch ---

fn dispatch_orchestrator_projects(
    action: &str,
    key: Option<String>,
    cwd: Option<String>,
    description: Option<String>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;
    match action {
        "list" => {
            let config = orchestrator::ProjectsConfig::load().map_err(DispatchError::Operation)?;
            let projects: Vec<Value> = config
                .list_resolved()
                .into_iter()
                .map(|p| json!({ "key": p.key, "cwd": p.cwd, "description": p.description }))
                .collect();
            Ok(json!({ "projects": projects }))
        }
        "add" => {
            let key = key.ok_or(DispatchError::InvalidParams("key を指定する".into()))?;
            let cwd = cwd.ok_or(DispatchError::InvalidParams("cwd を指定する".into()))?;
            orchestrator::ensure_defaults().map_err(DispatchError::Operation)?;
            let mut config =
                orchestrator::ProjectsConfig::load().map_err(DispatchError::Operation)?;
            config.add(key.clone(), cwd.clone(), description);
            config.save().map_err(DispatchError::Operation)?;
            Ok(json!({ "added": key, "cwd": cwd }))
        }
        "remove" => {
            let key = key.ok_or(DispatchError::InvalidParams("key を指定する".into()))?;
            let mut config =
                orchestrator::ProjectsConfig::load().map_err(DispatchError::Operation)?;
            if !config.remove(&key) {
                return Err(DispatchError::Operation(format!(
                    "プロジェクト '{key}' が見つからない"
                )));
            }
            config.save().map_err(DispatchError::Operation)?;
            Ok(json!({ "removed": key }))
        }
        _ => Err(DispatchError::InvalidParams(format!(
            "action が不正: {action}（list / add / remove）"
        ))),
    }
}

/// OrchestratorProfiles のパラメータ（Request と 1:1）。
/// ファイル直読みで tako-core の状態に依存しないため、CLI からも直接呼べるよう公開する
pub struct ProfilesParams {
    pub action: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub worker_model: Option<String>,
    pub effort: Option<String>,
    pub worker_effort: Option<String>,
    pub clear_model: bool,
    pub clear_worker_model: bool,
}

/// プロファイルを JSON 化する（list / show / set の共通形）。
/// model が null のときは claude CLI の既定モデルで起動することを表す
fn profile_to_json(name: &str, profile: &crate::orchestrator::Profile) -> Value {
    use crate::orchestrator;
    json!({
        "name": name,
        "model": profile.model,
        "effort": profile.effort,
        "worker_model_policy": profile.worker_model_policy,
        "worker_model": profile.worker_model,
        "worker_effort": profile.worker_effort,
        "resolved_worker_model": profile.resolve_worker_model(),
        "resolved_worker_effort": profile.resolve_worker_effort(),
        "path": orchestrator::profiles_dir()
            .map(|d| d.join(format!("{name}.yaml")).display().to_string()),
    })
}

/// プロファイル管理（list / show / set）。ファイル直読みなので tako-core の状態に依存しない
pub fn dispatch_orchestrator_profiles(params: ProfilesParams) -> Result<Value, DispatchError> {
    use crate::orchestrator;
    match params.action.as_str() {
        "list" => {
            let names = orchestrator::list_profiles().map_err(DispatchError::Operation)?;
            let profiles: Vec<Value> = names
                .iter()
                .map(|n| {
                    let p = orchestrator::Profile::load(n).unwrap_or_default();
                    profile_to_json(n, &p)
                })
                .collect();
            Ok(json!({ "profiles": profiles }))
        }
        "show" => {
            let name = params.name.as_deref().unwrap_or("default");
            let profile = match orchestrator::Profile::load(name) {
                Ok(p) => p,
                Err(_) if name == "default" => orchestrator::Profile::default(),
                Err(e) => return Err(DispatchError::Operation(e)),
            };
            Ok(profile_to_json(name, &profile))
        }
        "set" => {
            let name = params
                .name
                .ok_or(DispatchError::InvalidParams("name を指定する".into()))?;
            if params.model.is_some() && params.clear_model {
                return Err(DispatchError::InvalidParams(
                    "model と clear_model は同時に指定できない".into(),
                ));
            }
            if params.worker_model.is_some() && params.clear_worker_model {
                return Err(DispatchError::InvalidParams(
                    "worker_model と clear_worker_model は同時に指定できない".into(),
                ));
            }
            let mut profile = orchestrator::Profile::load(&name).unwrap_or_default();
            if let Some(m) = params.model {
                profile.model = Some(m);
            } else if params.clear_model {
                profile.model = None;
            }
            if let Some(m) = params.worker_model {
                profile.worker_model = Some(m);
            } else if params.clear_worker_model {
                profile.worker_model = None;
            }
            if let Some(e) = params.effort {
                profile.effort = e;
            }
            if let Some(e) = params.worker_effort {
                profile.worker_effort = Some(e);
            }
            let path = profile.save(&name).map_err(DispatchError::Operation)?;
            let mut result = profile_to_json(&name, &profile);
            result["path"] = json!(path.display().to_string());
            // [1m] は Max / API プラン限定 → 明示 opt-in は許容しつつ警告を返す
            // （inherit で master と同一モデルの場合は master 分のみ警告）
            let warnings: Vec<String> = [
                profile
                    .model
                    .as_deref()
                    .and_then(|m| orchestrator::one_m_model_warning(m, "master")),
                profile
                    .resolve_worker_model()
                    .filter(|m| Some(*m) != profile.model.as_deref())
                    .and_then(|m| orchestrator::one_m_model_warning(m, "worker")),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !warnings.is_empty() {
                result["warnings"] = json!(warnings);
            }
            Ok(result)
        }
        other => Err(DispatchError::InvalidParams(format!(
            "action が不正: {other}（list / show / set）"
        ))),
    }
}

/// tmux セッションへの送達確認つき配送をバックグラウンドスレッドで実行する
/// （Issue #32）。`deliver_via_tmux` は内部で sleep するブロッキング関数のため、
/// UI スレッド上の dispatch から直接呼ばない。結果はログのみ（fire-and-forget）
fn spawn_tmux_delivery(session: String, text: String, wait_ready: bool) {
    std::thread::spawn(move || {
        let socket = tako_core::tmux_backend::socket_name();
        match crate::claude_tui::deliver_via_tmux(Some(&socket), &session, &text, wait_ready) {
            Ok(report) if !report.verified => {
                eprintln!("warning: tmux 経由のプロンプト送達を検証できない（session={session}）");
            }
            Err(e) => {
                eprintln!("warning: tmux 経由のプロンプト送達に失敗（session={session}）: {e}");
            }
            Ok(_) => {}
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn dispatch_orchestrator_spawn(
    host: &mut dyn ControlHost,
    origin: PaneOrigin,
    project: &str,
    prompt: &str,
    label: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    pane: Option<u64>,
    tab: Option<u64>,
) -> Result<Value, DispatchError> {
    if pane.is_none() && tab.is_none() {
        return Err(DispatchError::Operation(
            "pane または tab を指定してください".into(),
        ));
    }

    use crate::orchestrator;

    let config = orchestrator::ProjectsConfig::load().map_err(DispatchError::Operation)?;
    let cwd = config
        .resolve_cwd(project)
        .map_err(DispatchError::Operation)?;

    // model/effort が明示指定されていない場合、呼び出し元 master のプロファイルから解決する。
    // model が None に解決された場合は --model を付けず claude CLI の既定に委ねる（Issue #27）
    let caller_pane = pane.map(PaneId::from_raw);
    let profile = resolve_caller_profile(host.workspace(), caller_pane);
    let model: Option<String> = match model {
        Some(m) => Some(m.to_string()),
        None => profile.resolve_worker_model().map(str::to_string),
    };
    let resolved_effort;
    let effort = match effort {
        Some(e) => e,
        None => {
            resolved_effort = profile.resolve_worker_effort().to_string();
            &resolved_effort
        }
    };
    let window_title = match label {
        Some(l) => format!("{project}: {l}"),
        None => format!("{project}-worker"),
    };

    // 分割元ペインの解決。優先順位: pane > tab > master role 検索
    let resolved_pane = pane.and_then(|p| resolve_pane(host.workspace(), Some(p)).ok());
    let (tab_id, target) = if let Some(resolved) = resolved_pane {
        resolved
    } else if let Some(raw_tab) = tab {
        let tid = find_tab(host.workspace(), raw_tab)?;
        let focused = host.workspace().get_tab(tid).unwrap().tree().focused();
        (tid, focused)
    } else {
        // master role のペインを検索。pane が指定されていても resolve に失敗した場合
        // （再起動で PaneId が変わった stale な値）はここに落ちる。
        // 複数 master 対応: caller の role suffix（例: ":tako"）と一致する master を優先。
        // stale な pane からは suffix を取れないため、全 master の中から orchestrator-worker
        // の spawned_by チェーンを遡って呼び出し元を推定する手段は無い。
        // → suffix マッチは pane が有効な場合のみ動作する
        let caller_suffix = resolved_pane
            .and_then(|(_, pid)| {
                host.workspace().tabs().iter().find_map(|t| {
                    t.tree()
                        .panes()
                        .iter()
                        .find(|pp| pp.id() == pid)
                        .and_then(|pp| pp.role())
                        .and_then(|r| r.strip_prefix("orchestrator-master"))
                        .map(|s| s.to_string())
                })
            })
            .unwrap_or_default();

        let find_master = |suffix: &str| -> Option<(TabId, PaneId)> {
            let target_role = format!("orchestrator-master{suffix}");
            host.workspace().tabs().iter().find_map(|t| {
                t.tree().panes().iter().find_map(|p| {
                    let role = p.role()?;
                    if role == target_role {
                        Some((t.id(), p.id()))
                    } else {
                        None
                    }
                })
            })
        };

        let master_pane = if !caller_suffix.is_empty() {
            find_master(&caller_suffix)
        } else {
            None
        }
        .or_else(|| {
            host.workspace().tabs().iter().find_map(|t| {
                t.tree().panes().iter().find_map(|p| {
                    let role = p.role()?;
                    if role.starts_with("orchestrator-master") {
                        Some((t.id(), p.id()))
                    } else {
                        None
                    }
                })
            })
        });

        master_pane.ok_or_else(|| {
            DispatchError::InvalidParams(
                "分割元ペインを特定できない（--pane または --tab を指定するか、tako 内のターミナルから実行する）".into(),
            )
        })?
    };
    let new_pane = Pane::new(origin);
    let new_id = new_pane.id();
    tree_mut(host.workspace_mut(), tab_id)
        .split_with_ratio(target, SplitDirection::Right, 0.45, new_pane)
        .map_err(op_err)?;
    // MCP/CLI 経由ではフォーカスを分割元に維持（ユーザーの入力を奪わない）
    let _ = tree_mut(host.workspace_mut(), tab_id).focus(target);
    let options = SpawnOptions {
        command: None,
        cwd: Some(std::path::PathBuf::from(&cwd)),
        env: Vec::new(),
    };
    host.attach_session(new_id, options);

    let role_value = match label {
        Some(l) => format!("worker:{project}:{l}"),
        None => format!("worker:{project}"),
    };
    let claude_cmd = orchestrator::build_worker_claude_cmd(&role_value, model.as_deref(), effort);

    // 事前信頼: 未信頼フォルダで claude を起動すると信頼ダイアログが出て、送信した
    // プロンプトがダイアログへの応答として消費される（Issue #32 問題 1）。起動前に
    // ~/.claude.json へ信頼済みを書き込んでダイアログ自体を出さない。失敗しても
    // PromptFlow のダイアログ検出 → 承諾がフォールバックするため継続する
    let pre_trusted = crate::claude_tui::ensure_trusted(&cwd).unwrap_or_else(|e| {
        eprintln!("warning: 事前信頼の書き込みに失敗（ダイアログ検出で継続）: {e}");
        false
    });

    // attach_session は非同期（pending_attach）なのでセッションはまだ存在しない。
    // queue_write で遅延書き込みを登録し、セッション起動後に自動送信する
    let mut cmd_bytes = claude_cmd.clone().into_bytes();
    cmd_bytes.push(b'\r');
    host.queue_write(new_id, cmd_bytes);

    // プロンプトは claude TUI の起動完了を画面内容で確認してから送達確認つきで送る。
    // ステートマシン駆動: alt_screen 遷移 → 信頼ダイアログ承諾 → ❯ 表示待ち →
    // bracketed paste → 分離 Enter → 入力欄の空検証 + Enter 再送（Issue #32）。
    // マルチラインは bracketed paste でそのまま渡るため改行の平坦化はしない
    host.queue_prompt_flow(new_id, prompt.to_string());

    // タイトルと role 設定
    let pane_obj = tree_mut(host.workspace_mut(), tab_id)
        .get_mut(new_id)
        .expect("直前に split で追加済み");
    pane_obj.set_title(Some(window_title.clone()));
    pane_obj.set_spawned_by(Some(target));
    let pane_role = match label {
        Some(l) => format!("orchestrator-worker:{project}:{l}"),
        None => format!("orchestrator-worker:{project}"),
    };
    pane_obj.set_role(Some(pane_role));

    let tmux_session = host.backend_session(new_id);

    Ok(json!({
        "pane_id": new_id.as_u64(),
        "spawned_by": target.as_u64(),
        "title": window_title,
        "cwd": cwd,
        "model": model,
        "effort": effort,
        "claude_command": claude_cmd,
        "prompt": prompt,
        "pre_trusted": pre_trusted,
        "tmux_session": tmux_session,
    }))
}

fn dispatch_orchestrator_worker_status(
    host: &dyn ControlHost,
    pane_id: u64,
    session_id: Option<&str>,
    tmux_session: Option<&str>,
) -> Result<Value, DispatchError> {
    use crate::orchestrator;

    // ペインの存在確認（ツリー上 + shelved の両方を走査）
    let target = PaneId::from_raw(pane_id);
    let in_tree = host.workspace().tabs().iter().any(|tab| {
        tab.tree()
            .panes()
            .iter()
            .any(|p| p.id().as_u64() == pane_id)
    });
    let pane_exists = in_tree || host.workspace().is_shelved(target);

    // session_id の解決: 明示指定 > pane→session 自動解決 > フォールバック
    let (resolved_sid, status_source);
    if let Some(sid) = session_id {
        resolved_sid = Some(sid.to_string());
        status_source = "agents";
    } else if pane_exists {
        // pane→session 自動解決: backend_session から pid 祖先辿り
        if let Some(backend) = host.backend_session(target) {
            if let Some(sid) = crate::agents::resolve_session_id_for_backend(&backend) {
                resolved_sid = Some(sid);
                status_source = "agents-auto";
            } else {
                resolved_sid = None;
                status_source = "screen";
            }
        } else {
            resolved_sid = None;
            status_source = "screen";
        }
    } else {
        resolved_sid = None;
        status_source = "none";
    };

    let (mut status, ctx_percent) = if let Some(ref sid) = resolved_sid {
        let agent = orchestrator::query_agent_status(sid);
        (agent.status, agent.ctx_percent)
    } else if pane_exists {
        ("unknown".to_string(), None)
    } else {
        ("gone".to_string(), None)
    };

    // ペインの最近の出力を取得（pane → tmux session フォールバック）
    let recent_output = host
        .session(target)
        .map(|session| {
            let mut lines = session.visible_lines();
            while lines.last().is_some_and(|l| l.is_empty()) {
                lines.pop();
            }
            if lines.len() > 30 {
                lines.drain(..lines.len() - 30);
            }
            lines.join("\n")
        })
        .or_else(|| {
            // tmux session フォールバック: pane が gone でも tmux session が生きていれば読む
            let ts = tmux_session?;
            let socket = tako_core::tmux_backend::socket_name();
            if !tako_core::tmux::session_alive(Some(&socket), ts) {
                return None;
            }
            // tmux session が生きている = pane は gone だが worker は生存中
            let mut lines = tako_core::tmux::capture_session(Some(&socket), ts).ok()?;
            while lines.last().is_some_and(|l| l.is_empty()) {
                lines.pop();
            }
            if lines.len() > 30 {
                lines.drain(..lines.len() - 30);
            }
            Some(lines.join("\n"))
        });

    // tmux session が生きていれば gone を取り消す（pane は無いが worker は健在）
    if status == "gone" {
        if let Some(ts) = tmux_session {
            let socket = tako_core::tmux_backend::socket_name();
            if tako_core::tmux::session_alive(Some(&socket), ts) {
                status = "unknown".to_string();
            }
        }
    }

    // idle 誤検知防止: サブエージェント完了の瞬間に claude agents --json が
    // 一時的に idle を返すことがある。末尾付近に ❯ プロンプトが
    // なければメインはまだ作業中なので busy に補正する
    // （判定は orchestrator::wait の完了監視ヒューリスティックと共通。#83）
    if status == "idle" {
        let has_prompt = recent_output
            .as_ref()
            .is_some_and(|out| crate::orchestrator::wait::screen_looks_idle(out));
        if !has_prompt {
            status = "busy".to_string();
        }
    }

    Ok(json!({
        "status": status,
        "ctx_percent": ctx_percent,
        "recent_output": recent_output,
        "status_source": status_source,
        "resolved_session_id": resolved_sid,
    }))
}

/// worker が busy かどうかを画面出力で判定する。
/// false negative より false positive を優先（殺すより残す方が安全）。
/// 判定は orchestrator::wait の完了監視ヒューリスティックと共通（#83）
fn is_worker_busy(host: &dyn ControlHost, target: PaneId) -> bool {
    let Some(session) = host.session(target) else {
        return true; // 画面取得不可 = busy 寄りに倒す
    };
    !crate::orchestrator::wait::screen_looks_idle(&session.visible_lines().join("\n"))
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '.' || c == '-' || c == '_')
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `setup_mcp_settings` の結果
pub struct SetupMcpResult {
    pub configured: bool,
    pub already_existed: bool,
}

/// Claude Code の settings.json に tako MCP サーバーの接続設定を追加する。
/// `tako_binary` は tako CLI のフルパス、`settings_path` は書き込む settings.json のパス。
/// 既に設定済みなら `already_existed=true`、新規追加なら `configured=true`
pub fn setup_mcp_settings(
    tako_binary: &str,
    settings_path: &std::path::Path,
) -> Result<SetupMcpResult, DispatchError> {
    let mut settings: serde_json::Map<String, Value> = if settings_path.is_file() {
        let content = std::fs::read_to_string(settings_path).map_err(|e| {
            DispatchError::Operation(format!("settings.json の読み取りに失敗: {e}"))
        })?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    let servers = settings.entry("mcpServers").or_insert_with(|| json!({}));
    if let Some(obj) = servers.as_object() {
        if obj.contains_key("tako") {
            return Ok(SetupMcpResult {
                configured: false,
                already_existed: true,
            });
        }
    }
    let servers_obj = servers.as_object_mut().ok_or_else(|| {
        DispatchError::Operation("settings.json の mcpServers がオブジェクトでない".into())
    })?;
    servers_obj.insert(
        "tako".to_string(),
        json!({
            "command": tako_binary,
            "args": ["mcp", "serve"],
        }),
    );
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            DispatchError::Operation(format!("{} の作成に失敗: {e}", parent.display()))
        })?;
    }
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| DispatchError::Operation(format!("JSON のシリアライズに失敗: {e}")))?;
    std::fs::write(settings_path, json).map_err(|e| {
        DispatchError::Operation(format!(
            "{} への書き込みに失敗: {e}",
            settings_path.display()
        ))
    })?;
    Ok(SetupMcpResult {
        configured: true,
        already_existed: false,
    })
}

/// tako CLI バイナリのパスを解決する。
/// ① `which tako`、② 実行中バイナリの隣（.app バンドル想定）、③ フォールバック "tako"
pub fn resolve_tako_binary() -> String {
    if let Some(path) = which("tako") {
        return path;
    }
    if let Ok(exe) = std::env::current_exe() {
        // .app バンドル: tako-app の隣に tako がある
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("tako");
            if sibling.is_file() {
                return sibling.display().to_string();
            }
        }
    }
    "tako".to_string()
}

fn check_health(host: &dyn ControlHost) -> Value {
    let app_version = env!("CARGO_PKG_VERSION").to_string();
    let mut issues: Vec<Value> = Vec::new();

    // tako CLI が PATH に通っているか
    let cli_path = which("tako");
    let cli_in_path = cli_path.is_some();
    if !cli_in_path {
        issues.push(json!({
            "level": "error",
            "check": "cli_in_path",
            "message": "tako CLI が PATH に見つからない。.app バンドル内の CLI を PATH に追加するか、\
                scripts/build-app.sh --install でインストールすること",
        }));
    }

    // CLI バージョンとアプリバージョンの一致
    let cli_version = cli_path
        .as_ref()
        .and_then(|path| {
            std::process::Command::new(path)
                .arg("--version")
                .output()
                .ok()
        })
        .and_then(|out| {
            String::from_utf8(out.stdout)
                .ok()
                .and_then(|s| s.split_whitespace().last().map(|v| v.to_string()))
        });
    let version_match = cli_version.as_deref() == Some(&app_version);
    if cli_in_path && !version_match {
        issues.push(json!({
            "level": "warning",
            "check": "version_match",
            "message": format!(
                "CLI バージョン ({}) とアプリバージョン ({}) が不一致。\
                 build-app.sh --install で最新の CLI をインストールすること",
                cli_version.as_deref().unwrap_or("不明"),
                app_version,
            ),
        }));
    }

    // tmux の有無
    let tmux_available = which("tmux").is_some();
    if !tmux_available {
        issues.push(json!({
            "level": "warning",
            "check": "tmux",
            "message": "tmux がインストールされていない。タブ構成の保存・復元は機能するが、\
                実行中プロセス・画面内容の復元（完全復元）は使えない。\
                brew install tmux でインストール可能",
        }));
    }

    // セッション永続化の状態
    let persist_enabled = host.tmux_persist_enabled();
    let persist_available = tako_core::tmux_backend::available();
    if tmux_available && !persist_enabled {
        issues.push(json!({
            "level": "info",
            "check": "persist",
            "message": "セッション永続化が無効。tako persist on で有効にすると、\
                tako 再起動時にプロセスと画面内容が復元される",
        }));
    }

    // ワークスペースの状態サマリ
    let ws = host.workspace();
    let tab_count = ws.tabs().len();
    let pane_count: usize = ws.tabs().iter().map(|t| t.tree().len()).sum();
    let bg_count = ws.shelved_panes().len();

    let healthy = issues.is_empty();

    json!({
        "healthy": healthy,
        "app_version": app_version,
        "cli_version": cli_version,
        "cli_in_path": cli_in_path,
        "version_match": version_match,
        "tmux_available": tmux_available,
        "persist_enabled": persist_enabled,
        "persist_available": persist_available,
        "workspace": {
            "tabs": tab_count,
            "panes": pane_count,
            "backgrounded": bg_count,
        },
        "issues": issues,
    })
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn which(name: &str) -> Option<String> {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `pane` 省略はエラー（呼び出し元解決はクライアント側の責務。FR-2.2.7）
fn resolve_pane(ws: &Workspace, pane: Option<u64>) -> Result<(TabId, PaneId), DispatchError> {
    let raw = pane.ok_or(DispatchError::NoTargetPane)?;
    for tab in ws.tabs() {
        if let Some(p) = tab.tree().panes().iter().find(|p| p.id().as_u64() == raw) {
            return Ok((tab.id(), p.id()));
        }
    }
    Err(DispatchError::PaneNotFound(raw))
}

fn find_tab(ws: &Workspace, raw: u64) -> Result<TabId, DispatchError> {
    ws.tabs()
        .iter()
        .map(|t| t.id())
        .find(|t| t.as_u64() == raw)
        .ok_or(DispatchError::TabNotFound(raw))
}

fn tree_mut(ws: &mut Workspace, tab: TabId) -> &mut tako_core::PaneTree {
    ws.get_tab_mut(tab)
        .expect("呼び出し前に存在確認済みのタブ")
        .tree_mut()
}

fn op_err(e: impl std::fmt::Display) -> DispatchError {
    DispatchError::Operation(e.to_string())
}

fn validate_name(name: &str) -> Result<(), DispatchError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(DispatchError::InvalidParams("無効なファイル名".into()));
    }
    Ok(())
}

fn dir_of(path: &std::path::Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf())
    }
}

/// ワークスペース全体の構造化スナップショット（FR-2.5.1〜2）。
/// ツリー構造 + 単位矩形ジオメトリ + 各ペインの状態を返す
fn list_json(host: &dyn ControlHost) -> Value {
    let ws = host.workspace();
    let tabs: Vec<Value> = ws
        .tabs()
        .iter()
        .map(|tab| {
            let tree = tab.tree();
            let rects = tree.layout(Rect::UNIT);
            // 前面表示中（アクティブタブ）か裏で動いているか（FR-2.16.12）。
            // tako はアクティブタブの全ペインをタイル表示するので、表示中 = アクティブタブ所属
            let tab_active = tab.id() == ws.active_tab_id();
            let panes: Vec<Value> = tree
                .panes()
                .iter()
                .map(|p| {
                    let rect = rects
                        .iter()
                        .find(|(id, _)| *id == p.id())
                        .map(|(_, r)| *r)
                        .expect("panes と layout は同じツリー由来");
                    let session = host.session(p.id());
                    json!({
                        "id": p.id().as_u64(),
                        // 表示分類（FR-2.16.12）。foreground = 前面表示中 / background = 裏で実行中
                        "surface": if tab_active { "foreground" } else { "background" },
                        "title": p.title(),
                        // title の出どころ（FR-2.12.3。manual は自動リネームに上書きされない）
                        "title_source": title_source_str(p.title_source()),
                        "osc_title": session.and_then(|s| s.title()),
                        "role": p.role(),
                        "spawned_by": p.spawned_by().map(|id| id.as_u64()),
                        "origin": origin_str(p.origin()),
                        "focused": p.id() == tree.focused(),
                        // OSC 7 / 133 シェル統合由来（未検知なら null / "unknown"。FR-2.1.4）
                        "cwd": session.and_then(|s| s.cwd()).map(|p| p.display().to_string()),
                        "state": session.map(|s| command_state_str(s.command_state())),
                        "exit_code": session.and_then(|s| match s.command_state() {
                            tako_core::CommandState::Failed(code) => Some(code),
                            _ => None,
                        }),
                        // ペイン配下プロセスの listen 中 TCP ポート（FR-2.4.2。
                        // tty 突き合わせのポーリング検知。未対応環境では空配列）
                        "listen_ports": session.map(|s| s.listen_ports().iter().map(|p| json!({
                            "port": p.port,
                            "pid": p.pid,
                            "process": p.process,
                        })).collect::<Vec<_>>()),
                        "rect": {
                            "x": rect.x,
                            "y": rect.y,
                            "width": rect.width,
                            "height": rect.height,
                        },
                        "cols": session.map(|s| s.size().0),
                        "rows": session.map(|s| s.size().1),
                        // スクロールバック表示の状態（FR-2.5.13。alt_screen 中は無効）
                        "scroll": session.map(|s| json!({
                            "offset": s.display_offset(),
                            "history": s.history_size(),
                            "alt_screen": s.is_alt_screen(),
                        })),
                        // プレビューペイン（FR-3.2 / FR-3.3）。ターミナルペインでは null
                        "preview": host.preview_state(p.id()).map(|(path, mode)| json!({
                            "path": path,
                            "mode": mode.as_str(),
                        })),
                        "backend_windows": host.backend_windows(p.id()).map(|ws| ws.iter().map(|w| json!({
                            "index": w.index,
                            "name": w.name,
                            "active": w.active,
                            "panes": w.panes,
                        })).collect::<Vec<_>>()),
                    })
                })
                .collect();
            json!({
                "id": tab.id().as_u64(),
                "title": tab.title(),
                "title_source": title_source_str(tab.title_source()),
                "active": tab_active,
                // サイドバー tmux ビューでこのタブ枠が折りたたまれているか（FR-2.16.14）
                "collapsed": host.tmux_tab_collapsed(tab.id()),
                "focused_pane": tree.focused().as_u64(),
                "panes": panes,
                "tree": tree_json(tree.root()),
            })
        })
        .collect();
    let shelved: Vec<Value> = ws
        .shelved_panes()
        .iter()
        .map(|bp| {
            json!({
                "id": bp.id().as_u64(),
                "title": bp.title(),
                "role": bp.role(),
                "origin": origin_str(bp.pane().origin()),
                "spawned_by": bp.pane().spawned_by().map(|id| id.as_u64()),
                "origin_tab": bp.origin_tab().as_u64(),
                "origin_tab_title": bp.origin_tab_title(),
            })
        })
        .collect();
    json!({
        "active_tab": ws.active_tab_id().as_u64(),
        "tabs": tabs,
        "shelved_panes": shelved,
        // ピン留め中のプレビューウィンドウ（FR-2.16.15。AI が現在のピンを把握できる）
        "pinned": pinned_json(host),
    })
}

/// ピン留め中のプレビュー一覧を JSON 配列へ（list / Pin 応答で共用。FR-2.16.15）
fn pinned_json(host: &dyn ControlHost) -> Value {
    Value::Array(
        host.pinned_previews()
            .into_iter()
            .map(|p| {
                json!({
                    "kind": if p.group { "group" } else { "pane" },
                    "id": p.id,
                    "x": p.x,
                    "y": p.y,
                })
            })
            .collect(),
    )
}

/// タイトルの出どころの文字列表現（list / MCP 公開用。FR-2.12.1）
fn title_source_str(source: tako_core::TitleSource) -> &'static str {
    match source {
        tako_core::TitleSource::Default => "default",
        tako_core::TitleSource::Auto => "auto",
        tako_core::TitleSource::Manual => "manual",
    }
}

/// コマンド実行状態の文字列表現（list / MCP 公開用）
fn command_state_str(state: tako_core::CommandState) -> &'static str {
    match state {
        tako_core::CommandState::Unknown => "unknown",
        tako_core::CommandState::Idle => "idle",
        tako_core::CommandState::Running => "running",
        tako_core::CommandState::Failed(_) => "failed",
    }
}

fn tree_json(node: &PaneNode) -> Value {
    match node {
        PaneNode::Leaf(p) => json!({ "type": "pane", "id": p.id().as_u64() }),
        PaneNode::Split {
            axis,
            ratio,
            first,
            second,
        } => json!({
            "type": "split",
            "axis": match axis {
                SplitAxis::Horizontal => "x",
                SplitAxis::Vertical => "y",
            },
            "ratio": ratio,
            "first": tree_json(first),
            "second": tree_json(second),
        }),
    }
}

fn origin_str(origin: PaneOrigin) -> &'static str {
    match origin {
        PaneOrigin::User => "user",
        PaneOrigin::Cli => "cli",
        PaneOrigin::Mcp => "mcp",
        PaneOrigin::Suggestion => "suggestion",
    }
}

/// UI スレッドで収集した pane/backend 対応表。`fetch_tmux_sessions` に渡す
pub struct TmuxContext {
    pub pane_of_tty: Vec<(String, u64, u64)>,
    pub backend_of: Vec<(String, u64, u64)>,
}

/// tmux セッション一覧を取得して JSON 配列を返す。
/// tmux コマンド実行（重い）を含むため、**background thread で呼ぶこと**。
/// dispatch の TmuxList と同じ JSON 構造を返す
pub fn fetch_tmux_sessions(ctx: &TmuxContext) -> Vec<Value> {
    let session_json = |s: &tako_core::TmuxSession, backend: bool, socket: &Value| {
        let clients: Vec<Value> = s
            .client_ttys
            .iter()
            .map(|tty| {
                let hit = ctx.pane_of_tty.iter().find(|(t, _, _)| t == tty);
                json!({
                    "tty": tty,
                    "pane": hit.map(|(_, pane, _)| *pane),
                    "tab": hit.map(|(_, _, tab)| *tab),
                })
            })
            .collect();
        let owner = ctx.backend_of.iter().find(|(name, _, _)| *name == s.name);
        json!({
            "name": s.name,
            "created": s.created,
            "attached": s.attached,
            "backend": backend,
            "socket": socket,
            "backend_pane": owner.map(|(_, pane, _)| *pane),
            "backend_tab": owner.map(|(_, _, tab)| *tab),
            "windows": s.windows.iter().map(|w| json!({
                "index": w.index,
                "name": w.name,
                "active": w.active,
                "panes": w.panes,
            })).collect::<Vec<_>>(),
            "clients": clients,
        })
    };
    let backend_socket = tako_core::tmux_backend::socket_name();
    let mut sessions: Vec<Value> = tako_core::tmux::list_sessions(None)
        .iter()
        .map(|s| session_json(s, false, &Value::Null))
        .collect();
    sessions.extend(
        tako_core::tmux::list_sessions(Some(&backend_socket))
            .iter()
            .map(|s| session_json(s, true, &backend_socket.clone().into())),
    );
    sessions
}

/// 呼び出し元ペインに紐づく master プロファイルを解決する。
/// caller の role（orchestrator-master:X）から直接、または spawned_by チェーンを辿って
/// master を見つけ、suffix からプロファイルを引く。
/// 見つからなければ default プロファイルにフォールバック。
fn resolve_caller_profile(
    workspace: &tako_core::Workspace,
    caller: Option<PaneId>,
) -> crate::orchestrator::Profile {
    // master が旧バージョンのまま再起動されていない場合でも spawn 経路で
    // 旧既定値 [1m] を引き継がないよう、読み込み前にマイグレーションする（Issue #27）
    let _ = crate::orchestrator::migrate_legacy_default_profile();
    let suffix = caller
        .and_then(|pid| find_master_suffix_from(workspace, pid))
        .unwrap_or_default();
    let name = if suffix.is_empty() {
        "default"
    } else {
        &suffix
    };
    crate::orchestrator::Profile::load(name).unwrap_or_default()
}

/// caller ペインから master の role suffix を検索する。
/// caller 自身が master なら直接返し、そうでなければ spawned_by を辿る。
fn find_master_suffix_from(workspace: &tako_core::Workspace, start: PaneId) -> Option<String> {
    if let Some(suffix) = pane_master_suffix(workspace, start) {
        return Some(suffix);
    }
    let mut current = start;
    for _ in 0..10 {
        let parent = workspace.tabs().iter().find_map(|t| {
            t.tree()
                .panes()
                .iter()
                .find(|p| p.id() == current)
                .and_then(|p| p.spawned_by())
        })?;
        if let Some(suffix) = pane_master_suffix(workspace, parent) {
            return Some(suffix);
        }
        current = parent;
    }
    None
}

fn pane_master_suffix(workspace: &tako_core::Workspace, pane_id: PaneId) -> Option<String> {
    workspace.tabs().iter().find_map(|t| {
        t.tree().panes().iter().find_map(|p| {
            if p.id() != pane_id {
                return None;
            }
            let role = p.role()?;
            if let Some(suffix) = role.strip_prefix("orchestrator-master:") {
                Some(suffix.to_string())
            } else if role == "orchestrator-master" {
                Some(String::new())
            } else {
                None
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Axis;

    /// セッションを起動しないテスト用ホスト（レイアウト操作の検証に使う）
    struct MockHost {
        ws: Workspace,
        attached: Vec<u64>,
        detached: Vec<u64>,
        previews: std::collections::HashMap<u64, (String, PreviewModeWire)>,
        collapsed: std::collections::HashSet<u64>,
        /// ピン留め: (group, id)
        pins: Vec<(bool, u64)>,
    }

    impl MockHost {
        fn new() -> Self {
            Self {
                ws: Workspace::new("t1", Pane::new(PaneOrigin::User)),
                attached: Vec::new(),
                detached: Vec::new(),
                previews: std::collections::HashMap::new(),
                collapsed: std::collections::HashSet::new(),
                pins: Vec::new(),
            }
        }

        fn toggle_pin(&mut self, group: bool, id: u64, pinned: Option<bool>) {
            let pos = self.pins.iter().position(|p| *p == (group, id));
            let want = pinned.unwrap_or(pos.is_none());
            match (want, pos) {
                (true, None) => self.pins.push((group, id)),
                (false, Some(i)) => {
                    self.pins.remove(i);
                }
                _ => {}
            }
        }

        fn root_pane(&self) -> u64 {
            self.ws.active_tab().tree().focused().as_u64()
        }
    }

    impl ControlHost for MockHost {
        fn workspace(&self) -> &Workspace {
            &self.ws
        }
        fn workspace_mut(&mut self) -> &mut Workspace {
            &mut self.ws
        }
        fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
            None
        }
        fn attach_session(&mut self, pane: PaneId, _options: SpawnOptions) {
            self.attached.push(pane.as_u64());
        }
        fn detach_session(&mut self, pane: PaneId) {
            self.detached.push(pane.as_u64());
            self.previews.remove(&pane.as_u64());
        }
        fn preview_state(&self, pane: PaneId) -> Option<(String, PreviewModeWire)> {
            self.previews.get(&pane.as_u64()).cloned()
        }
        fn set_preview(&mut self, pane: PaneId, path: &str, mode: PreviewModeWire) {
            self.previews.insert(pane.as_u64(), (path.into(), mode));
        }
        fn preview_pane_of_tab(&self, tab: TabId) -> Option<PaneId> {
            self.ws
                .get_tab(tab)?
                .tree()
                .panes()
                .into_iter()
                .map(|p| p.id())
                .find(|p| self.previews.contains_key(&p.as_u64()))
        }
        fn tmux_tab_collapsed(&self, tab: TabId) -> bool {
            self.collapsed.contains(&tab.as_u64())
        }
        fn set_tmux_tab_collapsed(&mut self, tab: TabId, collapsed: Option<bool>) {
            let now = collapsed.unwrap_or_else(|| !self.collapsed.contains(&tab.as_u64()));
            if now {
                self.collapsed.insert(tab.as_u64());
            } else {
                self.collapsed.remove(&tab.as_u64());
            }
        }
        fn pinned_previews(&self) -> Vec<PinnedView> {
            self.pins
                .iter()
                .map(|(group, id)| PinnedView {
                    group: *group,
                    id: *id,
                    x: 0.0,
                    y: 0.0,
                })
                .collect()
        }
        fn set_pin_pane(&mut self, pane: PaneId, pinned: Option<bool>) {
            self.toggle_pin(false, pane.as_u64(), pinned);
        }
        fn set_pin_group(&mut self, tab: TabId, pinned: Option<bool>) {
            self.toggle_pin(true, tab.as_u64(), pinned);
        }
    }

    fn split(host: &mut MockHost, pane: u64) -> u64 {
        dispatch(
            host,
            Request::Split {
                pane: Some(pane),
                tab: None,
                direction: None,
                ratio: None,
                command: None,
                cwd: None,
                focus: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap()["pane"]
            .as_u64()
            .unwrap()
    }

    #[test]
    fn splitで同じタブに新ペインが生えattachされる() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        assert_eq!(host.attached, vec![new_id]);
        assert_eq!(host.ws.active_tab().tree().len(), 2);
        // 生成主体は Cli（FR-2.3.5 のポリシー制御に使う）
        let tree = host.ws.active_tab().tree();
        let origin = tree
            .panes()
            .iter()
            .find(|p| p.id().as_u64() == new_id)
            .unwrap()
            .origin();
        assert_eq!(origin, PaneOrigin::Cli);
    }

    #[test]
    fn splitのtab指定は別タブ内に分割する() {
        let mut host = MockHost::new();
        let _root = host.root_pane();
        // タブ 2 を作り、タブ 1 に戻る
        let result = dispatch(&mut host, Request::TabNew { title: None }, PaneOrigin::Cli).unwrap();
        let tab2 = result["tab"].as_u64().unwrap();
        let tab2_pane = result["pane"].as_u64().unwrap();
        let tab1 = host.ws.tabs()[0].id().as_u64();
        dispatch(&mut host, Request::TabSelect { tab: tab1 }, PaneOrigin::Cli).unwrap();
        assert_eq!(host.ws.active_tab_id().as_u64(), tab1);
        // tab 指定でタブ 2 内に分割（active tab はタブ 1 のまま）
        let result = dispatch(
            &mut host,
            Request::Split {
                pane: None,
                tab: Some(tab2),
                direction: Some(Direction::Down),
                ratio: None,
                command: None,
                cwd: None,
                focus: None,
            },
            PaneOrigin::Mcp,
        )
        .unwrap();
        let new_pane = result["pane"].as_u64().unwrap();
        // 新ペインはタブ 2 内にある
        let t2 = host.ws.get_tab(find_tab(&host.ws, tab2).unwrap()).unwrap();
        assert_eq!(t2.tree().len(), 2);
        assert!(t2
            .tree()
            .panes()
            .iter()
            .any(|p| p.id().as_u64() == new_pane));
        // active tab は変わっていない
        assert_eq!(host.ws.active_tab_id().as_u64(), tab1);
        let _ = tab2_pane;
    }

    #[test]
    fn closeでペインが消えdetachされる() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        let result = dispatch(
            &mut host,
            Request::Close {
                pane: Some(new_id),
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["closed"].as_u64(), Some(new_id));
        assert_eq!(host.detached, vec![new_id]);
        assert_eq!(host.ws.active_tab().tree().len(), 1);
    }

    #[test]
    fn タブ最後のペインのcloseはタブごと閉じる() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        dispatch(&mut host, Request::TabNew { title: None }, PaneOrigin::Cli).unwrap();
        assert_eq!(host.ws.tabs().len(), 2);
        dispatch(
            &mut host,
            Request::Close {
                pane: Some(root),
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(host.ws.tabs().len(), 1);
        assert_eq!(host.detached, vec![root]);
    }

    #[test]
    fn 最後のタブの最後のペインは閉じられない() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let err = dispatch(
            &mut host,
            Request::Close {
                pane: Some(root),
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::Operation(_)));
        assert_eq!(host.ws.tabs().len(), 1);
        assert!(host.detached.is_empty());
    }

    #[test]
    fn focusはタブ切替も伴う() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let result = dispatch(&mut host, Request::TabNew { title: None }, PaneOrigin::Cli).unwrap();
        let tab2 = result["tab"].as_u64().unwrap();
        assert_eq!(host.ws.active_tab_id().as_u64(), tab2);
        // タブ 1 のペインへフォーカス → アクティブタブも戻る
        dispatch(
            &mut host,
            Request::Focus {
                pane: Some(root),
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_ne!(host.ws.active_tab_id().as_u64(), tab2);
        assert_eq!(host.ws.active_tab().tree().focused().as_u64(), root);
    }

    #[test]
    fn 方向フォーカスはアクティブタブ内で動く() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        // dispatch 経由の split はフォーカスを分割元（左側 = root）に維持する。
        // 右へ移動すると新ペインにフォーカスが移る
        let result = dispatch(
            &mut host,
            Request::Focus {
                pane: None,
                direction: Some(Direction::Right),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["focused"].as_u64(), Some(new_id));
        // 左へ戻ると root に戻る
        let result = dispatch(
            &mut host,
            Request::Focus {
                pane: None,
                direction: Some(Direction::Left),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["focused"].as_u64(), Some(root));
        // さらに左には何もない → null
        let result = dispatch(
            &mut host,
            Request::Focus {
                pane: None,
                direction: Some(Direction::Left),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(result["focused"].is_null());
    }

    #[test]
    fn resizeはdeltaとshareの排他指定() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        let result = dispatch(
            &mut host,
            Request::Resize {
                pane: Some(new_id),
                axis: Axis::X,
                delta: Some(0.2),
                share: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!((result["share"].as_f64().unwrap() - 0.7).abs() < 1e-5);
        let result = dispatch(
            &mut host,
            Request::Resize {
                pane: Some(new_id),
                axis: Axis::X,
                delta: None,
                share: Some(0.4),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!((result["share"].as_f64().unwrap() - 0.4).abs() < 1e-5);
        let err = dispatch(
            &mut host,
            Request::Resize {
                pane: Some(new_id),
                axis: Axis::X,
                delta: Some(0.1),
                share: Some(0.5),
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidParams(_)));
    }

    #[test]
    fn equalizeはpaneからタブを解決する() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        dispatch(
            &mut host,
            Request::Resize {
                pane: Some(new_id),
                axis: Axis::X,
                delta: Some(0.3),
                share: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        dispatch(
            &mut host,
            Request::Equalize {
                pane: Some(root),
                tab: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let rects = host.ws.active_tab().tree().layout(Rect::UNIT);
        for (_, r) in rects {
            assert!((r.width - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn listはペインの表示分類surfaceを返す() {
        // FR-2.16.12: 表示中 = アクティブタブ所属、それ以外は裏で実行中
        let mut host = MockHost::new();
        let root = host.root_pane(); // t1 のペイン
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User)); // t2 がアクティブに
        let result = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let tabs = result["tabs"].as_array().unwrap();
        for tab in tabs {
            let active = tab["active"].as_bool().unwrap();
            for p in tab["panes"].as_array().unwrap() {
                let surface = p["surface"].as_str().unwrap();
                let want = if active { "foreground" } else { "background" };
                assert_eq!(surface, want);
            }
        }
        // root（非アクティブな t1）は background
        let root_surface = tabs
            .iter()
            .flat_map(|t| t["panes"].as_array().unwrap())
            .find(|p| p["id"].as_u64() == Some(root))
            .unwrap()["surface"]
            .as_str()
            .unwrap();
        assert_eq!(root_surface, "background");
    }

    #[test]
    fn backgroundリストは由来タブとbackgroundを返す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let t1 = host.ws.active_tab_id();
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User));
        dispatch(
            &mut host,
            Request::Background { pane: Some(root) },
            PaneOrigin::Cli,
        )
        .unwrap();
        let result = dispatch(&mut host, Request::BackgroundList, PaneOrigin::Cli).unwrap();
        let items = result["backgrounded"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["pane"].as_u64(), Some(root));
        assert_eq!(items[0]["origin_tab"].as_u64(), Some(t1.as_u64()));
        assert_eq!(items[0]["origin_tab_title"].as_str(), Some("t1"));
        assert_eq!(items[0]["surface"].as_str(), Some("background"));
    }

    #[test]
    fn foregroundはtarget省略で由来タブへ戻す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let t1 = host.ws.active_tab_id();
        let p2 = split(&mut host, root);
        host.ws.create_tab("t2", Pane::new(PaneOrigin::User));
        dispatch(
            &mut host,
            Request::Background { pane: Some(p2) },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(host.ws.is_shelved(PaneId::from_raw(p2)));
        let result = dispatch(
            &mut host,
            Request::Foreground {
                pane: p2,
                target: None,
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["foregrounded"].as_u64(), Some(p2));
        assert!(!host.ws.is_shelved(PaneId::from_raw(p2)));
        assert_eq!(host.ws.find_tab_of_pane(PaneId::from_raw(p2)), Some(t1));
    }

    #[test]
    fn collapsetabはトグルとset両方ができ_listに出る() {
        // FR-2.16.14: 折りたたみは tab 指定 / 呼び出し元タブの両方で操作でき、
        // collapsed 省略でトグル、list の各タブ collapsed で状態取得できる
        let mut host = MockHost::new();
        let t1 = host.ws.active_tab_id();
        // 初期は折りたたまれていない
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["tabs"][0]["collapsed"].as_bool(), Some(false));
        // collapsed 省略 = トグルで折りたたむ
        let r = dispatch(
            &mut host,
            Request::CollapseTab {
                pane: None,
                tab: Some(t1.as_u64()),
                collapsed: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r["collapsed"].as_bool(), Some(true));
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["tabs"][0]["collapsed"].as_bool(), Some(true));
        // collapsed=false で明示展開
        let r = dispatch(
            &mut host,
            Request::CollapseTab {
                pane: None,
                tab: Some(t1.as_u64()),
                collapsed: Some(false),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(r["collapsed"].as_bool(), Some(false));
        // tab 省略時は pane（呼び出し元）の属するタブを畳む
        let root = host.root_pane();
        dispatch(
            &mut host,
            Request::CollapseTab {
                pane: Some(root),
                tab: None,
                collapsed: Some(true),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert!(host.tmux_tab_collapsed(t1));
    }

    #[test]
    fn pinはトグルとunpinができ_listのpinnedに出る() {
        // FR-2.16.15: pane のピン留め / 解除が list の pinned に反映される
        let mut host = MockHost::new();
        let root = host.root_pane();
        // 初期は空
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["pinned"].as_array().unwrap().len(), 0);
        // pinned 省略 = トグルでピン留め
        dispatch(
            &mut host,
            Request::Pin {
                pane: Some(root),
                group_tab: None,
                pinned: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let pinned = list["pinned"].as_array().unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0]["kind"].as_str(), Some("pane"));
        assert_eq!(pinned[0]["id"].as_u64(), Some(root));
        // pinned=false で解除
        dispatch(
            &mut host,
            Request::Pin {
                pane: Some(root),
                group_tab: None,
                pinned: Some(false),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["pinned"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn pinのgroup_tabはバックグラウンドの由来が無いと弾く() {
        // 閉じたタブグループのピンは、その由来を持つバックグラウンドペインが居るときだけ通る
        let mut host = MockHost::new();
        let err = dispatch(
            &mut host,
            Request::Pin {
                pane: None,
                group_tab: Some(9999),
                pinned: Some(true),
            },
            PaneOrigin::Cli,
        );
        assert!(matches!(err, Err(DispatchError::TabNotFound(9999))));
    }

    #[test]
    fn listはツリーとジオメトリと状態を返す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        dispatch(
            &mut host,
            Request::Title {
                pane: Some(new_id),
                title: Some("worker".into()),
                role: Some("dev-server".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let result = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let tabs = result["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 1);
        let panes = tabs[0]["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 2);
        let new_pane = panes
            .iter()
            .find(|p| p["id"].as_u64() == Some(new_id))
            .unwrap();
        assert_eq!(new_pane["title"].as_str(), Some("worker"));
        assert_eq!(new_pane["role"].as_str(), Some("dev-server"));
        assert_eq!(new_pane["origin"].as_str(), Some("cli"));
        // dispatch 経由の split はフォーカスを移さない（分割元を維持）
        assert_eq!(new_pane["focused"].as_bool(), Some(false));
        assert!((new_pane["rect"]["x"].as_f64().unwrap() - 0.5).abs() < 1e-5);
        // ツリー構造（ルートが split で leaf を 2 つ持つ）
        assert_eq!(tabs[0]["tree"]["type"].as_str(), Some("split"));
        assert_eq!(tabs[0]["tree"]["second"]["id"].as_u64(), Some(new_id));
    }

    #[test]
    fn sendとreadはセッションが無ければエラー() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let err = dispatch(
            &mut host,
            Request::Send {
                pane: Some(root),
                text: "ls".into(),
                newline: true,
                tmux_session: None,
                await_prompt: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::NoSession(root));
        let err = dispatch(
            &mut host,
            Request::Read {
                pane: Some(root),
                lines: None,
                tmux_session: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::NoSession(root));
    }

    #[test]
    fn タブ操作とペイン移送() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root);
        let result = dispatch(
            &mut host,
            Request::TabNew {
                title: Some("agents".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab2 = result["tab"].as_u64().unwrap();
        // TabNew のペインも attach される
        assert_eq!(host.attached.len(), 2);

        dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(new_id),
                tab: Some(tab2),
                target: None,
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            host.ws
                .find_tab_of_pane(
                    host.ws
                        .get_tab(find_tab(&host.ws, tab2).unwrap())
                        .unwrap()
                        .tree()
                        .focused()
                )
                .unwrap()
                .as_u64(),
            tab2
        );
        assert_eq!(
            host.ws
                .get_tab(find_tab(&host.ws, tab2).unwrap())
                .unwrap()
                .tree()
                .len(),
            2
        );

        // タブ切替
        let tab1 = host.ws.tabs()[0].id().as_u64();
        dispatch(&mut host, Request::TabSelect { tab: tab1 }, PaneOrigin::Cli).unwrap();
        assert_eq!(host.ws.active_tab_id().as_u64(), tab1);
    }

    #[test]
    fn move_paneのtarget指定は同タブ内で挿し直す() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let new_id = split(&mut host, root); // [root | new]
                                             // root を new の下へ（FR-1.10 = タイトルバー D&D の同等操作）
        dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: None,
                target: Some(new_id),
                direction: Some(Direction::Down),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let rects = host.ws.active_tab().tree().layout(Rect::UNIT);
        let rect_of = |raw: u64| {
            rects
                .iter()
                .find(|(p, _)| p.as_u64() == raw)
                .map(|(_, r)| *r)
                .unwrap()
        };
        assert!(rect_of(new_id).y < rect_of(root).y, "root が下に入る");
        assert!((rect_of(root).width - 1.0).abs() < 1e-5, "縦分割 = 全幅");
        // tab と target の同時指定・両方省略・target + tab なし direction はエラー
        let tab1 = host.ws.tabs()[0].id().as_u64();
        let err = dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: Some(tab1),
                target: Some(new_id),
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidParams(_)));
        let err = dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: None,
                target: None,
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidParams(_)));
        let err = dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: Some(tab1),
                target: None,
                direction: Some(Direction::Down),
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::InvalidParams(_)));
        // 自分自身へはドメイン層が拒否する
        let err = dispatch(
            &mut host,
            Request::MovePane {
                pane: Some(root),
                tab: None,
                target: Some(root),
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::Operation(_)));
    }

    #[test]
    fn タブのリネームと手動優先() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        // pane からタブを解決してリネーム（FR-2.12.1）
        let result = dispatch(
            &mut host,
            Request::TabRename {
                pane: Some(root),
                tab: None,
                title: "実験".into(),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(result["title"].as_str(), Some("実験"));
        let tab = &host.ws.tabs()[0];
        assert_eq!(tab.title(), "実験");
        assert_eq!(tab.title_source(), tako_core::TitleSource::Manual);
        // list に title_source が公開される
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        assert_eq!(list["tabs"][0]["title_source"].as_str(), Some("manual"));
        assert_eq!(
            list["tabs"][0]["panes"][0]["title_source"].as_str(),
            Some("default")
        );
        // 空文字で手動指定を解除（タイトルは保持）
        let tab_id = host.ws.tabs()[0].id().as_u64();
        dispatch(
            &mut host,
            Request::TabRename {
                pane: None,
                tab: Some(tab_id),
                title: String::new(),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        let tab = &host.ws.tabs()[0];
        assert_eq!(tab.title(), "実験");
        assert_eq!(tab.title_source(), tako_core::TitleSource::Default);
    }

    #[test]
    fn 明示タイトル付きのタブ作成は手動扱い() {
        let mut host = MockHost::new();
        dispatch(
            &mut host,
            Request::TabNew {
                title: Some("agents".into()),
            },
            PaneOrigin::Cli,
        )
        .unwrap();
        assert_eq!(
            host.ws.active_tab().title_source(),
            tako_core::TitleSource::Manual
        );
        // 連番の既定タイトルは Default のまま（自動リネーム対象）
        dispatch(&mut host, Request::TabNew { title: None }, PaneOrigin::Cli).unwrap();
        assert_eq!(
            host.ws.active_tab().title_source(),
            tako_core::TitleSource::Default
        );
    }

    #[test]
    fn open_fileはプレビューペインを生やし再利用する() {
        let dir = std::env::temp_dir().join(format!("tako-dispatch-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("b.md"), "# 見出し").unwrap();

        let mut host = MockHost::new();
        let root = host.root_pane();
        let open = |host: &mut MockHost, path: String, mode: Option<PreviewModeWire>| {
            dispatch(
                host,
                Request::OpenFile {
                    pane: Some(root),
                    path,
                    mode,
                    direction: None,
                },
                PaneOrigin::Mcp,
            )
        };
        // 新設: ペインが生え、ターミナルは attach されない。mode は拡張子から code
        let result = open(&mut host, dir.join("a.rs").display().to_string(), None).unwrap();
        let preview_pane = result["pane"].as_u64().unwrap();
        assert_ne!(preview_pane, root);
        assert_eq!(result["created"].as_bool(), Some(true));
        assert_eq!(result["mode"].as_str(), Some("code"));
        assert!(host.attached.is_empty(), "プレビューは PTY を起動しない");
        assert_eq!(host.ws.active_tab().tree().len(), 2);
        // フォーカスはプレビューペインへ
        assert_eq!(host.ws.active_tab().tree().focused().as_u64(), preview_pane);
        // 再利用: 同タブの 2 ファイル目は同じペインに差し替わる。.md は markdown 既定
        let result = open(&mut host, dir.join("b.md").display().to_string(), None).unwrap();
        assert_eq!(result["pane"].as_u64(), Some(preview_pane));
        assert_eq!(result["created"].as_bool(), Some(false));
        assert_eq!(result["mode"].as_str(), Some("markdown"));
        assert_eq!(host.ws.active_tab().tree().len(), 2);
        // mode の明示指定（トグルの CLI / MCP 経路）: 同じファイルを code 表示へ
        let result = open(
            &mut host,
            dir.join("b.md").display().to_string(),
            Some(PreviewModeWire::Code),
        )
        .unwrap();
        assert_eq!(result["mode"].as_str(), Some("code"));
        // list に preview が公開される
        let list = dispatch(&mut host, Request::List, PaneOrigin::Cli).unwrap();
        let panes = list["tabs"][0]["panes"].as_array().unwrap();
        let preview = panes
            .iter()
            .find(|p| p["id"].as_u64() == Some(preview_pane))
            .unwrap();
        assert_eq!(preview["preview"]["mode"].as_str(), Some("code"));
        assert!(preview["preview"]["path"]
            .as_str()
            .unwrap()
            .ends_with("b.md"));
        // 存在しないパス・ディレクトリはエラー
        assert!(open(&mut host, dir.join("no-such").display().to_string(), None).is_err());
        assert!(open(&mut host, dir.display().to_string(), None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_fileのdirection指定は再利用せず分割する() {
        let dir =
            std::env::temp_dir().join(format!("tako-dispatch-open-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();

        let mut host = MockHost::new();
        let root = host.root_pane();
        let open = |host: &mut MockHost, direction: Option<Direction>| {
            dispatch(
                host,
                Request::OpenFile {
                    pane: Some(root),
                    path: dir.join("a.rs").display().to_string(),
                    mode: None,
                    direction,
                },
                PaneOrigin::User,
            )
            .unwrap()
        };
        // 1 枚目（direction なし）でプレビューが生える
        let first = open(&mut host, None)["pane"].as_u64().unwrap();
        // direction 指定（D&D のドロップ位置。FR-3.11）は既存プレビューを再利用しない
        let result = open(&mut host, Some(Direction::Down));
        let second = result["pane"].as_u64().unwrap();
        assert_ne!(second, first, "再利用せず新ペインに開く");
        assert_eq!(result["created"].as_bool(), Some(true));
        assert_eq!(host.ws.active_tab().tree().len(), 3);
        assert!(host.attached.is_empty(), "プレビューは PTY を起動しない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tmux_openは存在しないセッションを分割前に弾く() {
        let mut host = MockHost::new();
        let root = host.root_pane();
        let err = dispatch(
            &mut host,
            Request::TmuxOpen {
                socket: Some(format!("tako-test-no-such-server-{}", std::process::id())),
                session: "no-such-session".into(),
                window: None,
                pane: Some(root),
                direction: None,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert!(matches!(err, DispatchError::Operation(_)));
        // 分割もセッション起動も起きていない
        assert_eq!(host.ws.active_tab().tree().len(), 1);
        assert!(host.attached.is_empty());
    }

    #[test]
    fn 不正な対象はエラー() {
        let mut host = MockHost::new();
        let err = dispatch(
            &mut host,
            Request::Close {
                pane: None,
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::NoTargetPane);
        let err = dispatch(
            &mut host,
            Request::Close {
                pane: Some(99999),
                force: false,
            },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::PaneNotFound(99999));
        let err = dispatch(
            &mut host,
            Request::TabSelect { tab: 99999 },
            PaneOrigin::Cli,
        )
        .unwrap_err();
        assert_eq!(err, DispatchError::TabNotFound(99999));
    }
}

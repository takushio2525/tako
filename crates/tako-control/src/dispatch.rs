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
    /// ペインを保持している tmux バックエンドセッション名（tmuxview の区別表示用。
    /// バックエンドでないペイン・非対応実装では None）
    fn backend_session(&self, _pane: PaneId) -> Option<String> {
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
    /// たまり場から復帰させたペインのセッションを再接続する（FR-2.15.3）。
    /// セッション自体は shelve 時に破棄していないため、UI 層で再描画するだけでよい場合が多い
    fn reattach_shelved(&mut self, _pane: PaneId) {}
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
    /// 退避ペイン・表示中ビューを protected として除外し、backend socket 上の取り残し
    /// セッションを kill する。kill した名前を返す
    fn cleanup_orphan_tmux(&self) -> Vec<String> {
        Vec::new()
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
            Ok(json!({ "pane": new_id.as_u64() }))
        }

        Request::Close { pane } => {
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
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
        } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let session = host
                .session(target)
                .ok_or(DispatchError::NoSession(target.as_u64()))?;
            let mut bytes = text.into_bytes();
            if newline {
                bytes.push(b'\r');
            }
            session.write(bytes);
            Ok(Value::Null)
        }

        Request::Read { pane, lines } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            let session = host
                .session(target)
                .ok_or(DispatchError::NoSession(target.as_u64()))?;
            let mut all = session.visible_lines();
            while all.last().is_some_and(|l| l.is_empty()) {
                all.pop();
            }
            if let Some(n) = lines {
                if all.len() > n {
                    all.drain(..all.len() - n);
                }
            }
            Ok(json!({ "pane": target.as_u64(), "text": all.join("\n") }))
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
            // 元セッションの解決（無限ネスト防止 = 今回の根治）。tmux はグループ名を
            // 「最初に作られた元セッション名」にするため、`tako-view-*` ラッパーや grouped
            // session を開こうとしても group を辿れば必ず元へ戻る。
            // 例: `tako-view-tako-view-master-tako-2-0`（group=master-tako）→ `master-tako`
            let group = tako_core::tmux::session_group(socket.as_deref(), &session);
            let original = group.unwrap_or_else(|| session.clone());
            // tako 自身が作ったラッパーを開き直す場合（退避からの復帰・再オープン等）は、
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
            let title = title.unwrap_or_else(|| format!("{}", host.workspace().tabs().len() + 1));
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
                // tmux 不在環境では enabled でも直接 spawn へ劣化していることを示す
                "available": tako_core::tmux_backend::available(),
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
            let (tab, target) = resolve_pane(host.workspace(), pane)?;
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
                    let dir = if path.is_dir() {
                        path.clone()
                    } else {
                        path.parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or(path.clone())
                    };
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
                    if new_name.is_empty() || new_name.contains('/') || new_name.contains('\\') {
                        return Err(DispatchError::InvalidParams("無効なファイル名".into()));
                    }
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
                    if file_name.is_empty() || file_name.contains('/') || file_name.contains('\\') {
                        return Err(DispatchError::InvalidParams("無効なファイル名".into()));
                    }
                    let parent = if path.is_dir() {
                        path.clone()
                    } else {
                        path.parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or(path.clone())
                    };
                    let new_path = parent.join(&file_name);
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
                    if dir_name.is_empty() || dir_name.contains('/') || dir_name.contains('\\') {
                        return Err(DispatchError::InvalidParams("無効なフォルダ名".into()));
                    }
                    let parent = if path.is_dir() {
                        path.clone()
                    } else {
                        path.parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or(path.clone())
                    };
                    let new_path = parent.join(&dir_name);
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

        Request::Shelve { pane } => {
            let (_, target) = resolve_pane(host.workspace(), pane)?;
            host.workspace_mut().shelve_pane(target).map_err(op_err)?;
            Ok(json!({ "shelved": target.as_u64() }))
        }

        Request::Unshelve {
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
                host.workspace().active_tab().tree().focused()
            };
            let dir = direction
                .map(|d| d.to_core())
                .unwrap_or(SplitDirection::Right);
            host.workspace_mut()
                .unshelve_pane(pane_id, target_id, dir)
                .map_err(op_err)?;
            host.reattach_shelved(pane_id);
            Ok(json!({ "unshelved": pane, "target": target_id.as_u64() }))
        }

        Request::ShelvedList => {
            let shelved: Vec<serde_json::Value> = host
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
                    })
                })
                .collect();
            Ok(json!({ "shelved": shelved }))
        }

        Request::ShelvedKill { pane } => {
            let pane_id = PaneId::from_raw(pane);
            if host.workspace_mut().remove_shelved(pane_id).is_none() {
                return Err(DispatchError::PaneNotFound(pane));
            }
            host.detach_session(pane_id);
            Ok(json!({ "killed": pane }))
        }

        Request::CheckHealth => Ok(check_health(host)),
    }
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
            "message": "tmux がインストールされていない。セッション永続化（tako 再起動時の復元）が\
                使えない。brew install tmux でインストール可能",
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
    let shelved_count = ws.shelved_panes().len();

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
            "shelved": shelved_count,
        },
        "issues": issues,
    })
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
                        "title": p.title(),
                        // title の出どころ（FR-2.12.3。manual は自動リネームに上書きされない）
                        "title_source": title_source_str(p.title_source()),
                        "osc_title": session.and_then(|s| s.title()),
                        "role": p.role(),
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
                    })
                })
                .collect();
            json!({
                "id": tab.id().as_u64(),
                "title": tab.title(),
                "title_source": title_source_str(tab.title_source()),
                "active": tab.id() == ws.active_tab_id(),
                "focused_pane": tree.focused().as_u64(),
                "panes": panes,
                "tree": tree_json(tree.root()),
            })
        })
        .collect();
    json!({ "active_tab": ws.active_tab_id().as_u64(), "tabs": tabs })
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
    }

    impl MockHost {
        fn new() -> Self {
            Self {
                ws: Workspace::new("t1", Pane::new(PaneOrigin::User)),
                attached: Vec::new(),
                detached: Vec::new(),
                previews: std::collections::HashMap::new(),
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
            Request::Close { pane: Some(new_id) },
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
            Request::Close { pane: Some(root) },
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
            Request::Close { pane: Some(root) },
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
        // split 後のフォーカスは新ペイン（右側）。左へ戻る
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
        let _ = new_id;
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
        assert_eq!(new_pane["focused"].as_bool(), Some(true));
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
        let err = dispatch(&mut host, Request::Close { pane: None }, PaneOrigin::Cli).unwrap_err();
        assert_eq!(err, DispatchError::NoTargetPane);
        let err = dispatch(
            &mut host,
            Request::Close { pane: Some(99999) },
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

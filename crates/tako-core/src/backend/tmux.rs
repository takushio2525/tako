//! TmuxBackend — tmux を器とするバックエンド（macOS / Linux の既定）
//!
//! 段取り ①（#519）では **既存の `crate::tmux_backend` / `crate::tmux` の自由関数へ
//! 委譲する薄いアダプタ**である。tmux コマンド組み立てのコードは物理的にはまだ
//! 元の位置にあり、呼び出し側も従来どおり自由関数を直接呼んでいる。
//!
//! この形を選んだ理由: 段取り ① の目的は「境界を作ること」であって
//! 「ファイルを動かすこと」ではない。1400 行 + 740 行の移設を先にやると、
//! 挙動不変の証明（差分レビュー）が実質不可能な大きさになり、
//! 並行 worker（`dispatch.rs` / `platform/`）との衝突面積も無用に広がる。
//! 物理的な移設は、役割 B の切り出しで関数群が役割ごとに分かれる段取り ③ で行う。
//!
//! **挙動不変の担保**: `wrap_spawn` が生成する argv は
//! `tmux_backend::wrap_options` と同一である（`tests::wrap_spawn_argv_スナップショット`
//! が全語を固定しており、順序・有無が変わると落ちる）。

use std::collections::HashSet;
use std::time::Duration;

use super::{
    BackendCapabilities, BackendError, DetachedAccess, HistoryProbe, Holder, ScrollbackAuthority,
    SessionBackend, SessionInfo, SessionRef,
};
use crate::terminal::SpawnOptions;

pub struct TmuxBackend {
    /// 専用サーバーのソケット名（`tmux -L`）。プロセス内で固定する
    socket: String,
}

impl TmuxBackend {
    pub fn new() -> Self {
        Self {
            socket: crate::tmux_backend::socket_name(),
        }
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }

    fn sock(&self) -> Option<&str> {
        Some(self.socket.as_str())
    }
}

impl Default for TmuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// `HashSet<SessionRef>` → 既存 API が期待する `HashSet<String>`
fn to_name_set(set: &HashSet<SessionRef>) -> HashSet<String> {
    set.iter().map(|s| s.as_str().to_string()).collect()
}

/// 既存 API が返す名前を `SessionRef` へ戻す。tmux が返す名前は
/// 定義上ターゲット式ではないので、万一壊れた行が来たら黙って捨てる
fn to_refs(names: Vec<String>) -> Vec<SessionRef> {
    names
        .into_iter()
        .filter_map(|n| SessionRef::new(n).ok())
        .collect()
}

impl SessionBackend for TmuxBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            survives_app_exit: true,
            detached_access: true,
            scrollback: ScrollbackAuthority::Backend,
            label: "tmux",
        }
    }

    fn reserve(&self, candidate: &str) -> Option<SessionRef> {
        SessionRef::new(candidate).ok()
    }

    fn wrap_spawn(&self, options: SpawnOptions, session: &SessionRef) -> SpawnOptions {
        crate::tmux_backend::wrap_options(options, &self.socket, session.as_str())
    }

    fn exists(&self, session: &SessionRef) -> bool {
        crate::tmux::has_session(self.sock(), session.as_str())
    }

    fn kill(&self, session: &SessionRef) -> Result<(), BackendError> {
        // 既存の `tmux_backend::kill_session` と同じく「既に無い」は無害として潰す
        crate::tmux_backend::kill_session(&self.socket, session.as_str());
        Ok(())
    }

    fn list(&self) -> Vec<SessionInfo> {
        crate::tmux::list_sessions(self.sock())
            .into_iter()
            .filter_map(|s| {
                Some(SessionInfo {
                    session: SessionRef::new(s.name).ok()?,
                    attached: s.attached,
                    last_activity: s.last_activity,
                })
            })
            .collect()
    }

    fn foreign_holders(&self, sessions: &[SessionRef]) -> Vec<Holder> {
        if sessions.is_empty() {
            return Vec::new();
        }
        let wanted: HashSet<&str> = sessions.iter().map(SessionRef::as_str).collect();
        crate::tmux::list_client_pids(self.sock())
            .into_iter()
            .filter(|(_, name)| wanted.contains(name.as_str()))
            .filter_map(|(pid, name)| {
                Some(Holder {
                    pid,
                    session: SessionRef::new(name).ok()?,
                })
            })
            .collect()
    }

    fn orphans(&self, protected: &HashSet<SessionRef>) -> Vec<SessionRef> {
        to_refs(crate::tmux_backend::find_orphans(
            &self.socket,
            &to_name_set(protected),
        ))
    }

    fn cleanup_orphans(
        &self,
        protected: &HashSet<SessionRef>,
        min_idle: Option<Duration>,
    ) -> Vec<SessionRef> {
        to_refs(crate::tmux_backend::cleanup_orphans(
            &self.socket,
            &to_name_set(protected),
            min_idle.map(|d| d.as_secs()),
        ))
    }

    fn pane_tty(&self, session: &SessionRef) -> Option<String> {
        crate::tmux_backend::pane_tty(&self.socket, session.as_str())
    }

    fn session_env(&self, session: &SessionRef, name: &str) -> Option<String> {
        crate::tmux_backend::session_env(&self.socket, session.as_str(), name)
    }

    fn set_session_env(&self, session: &SessionRef, name: &str, value: &str) {
        crate::tmux_backend::set_session_env(&self.socket, session.as_str(), name, value);
    }

    fn sync_config(&self) {
        crate::tmux_backend::sync_conf(&self.socket);
    }

    fn detached(&self) -> Option<&dyn DetachedAccess> {
        Some(self)
    }
}

impl DetachedAccess for TmuxBackend {
    fn capture_screen(&self, session: &SessionRef) -> Result<Vec<String>, BackendError> {
        crate::tmux::capture_session(self.sock(), session.as_str()).map_err(BackendError::Operation)
    }

    fn capture_history(&self, session: &SessionRef, lines: usize) -> Option<Vec<String>> {
        crate::tmux::capture_history_plain(self.sock(), session.as_str(), lines)
    }

    fn capture_history_joined(&self, session: &SessionRef, lines: usize) -> Option<String> {
        // `-J` で折り返し行を結合する。`capture_history`（`-J` 無し）とは別物
        let start = format!("-{lines}");
        let output = crate::tmux::tmux_command(self.sock())
            .args([
                "capture-pane",
                "-p",
                "-J",
                "-t",
                &format!("={}:", session.as_str()),
                "-S",
                &start,
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        // 行単位で組み直してから末尾を落とす（CRLF 正規化。移設元と同一）
        let text = String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn history_probe(&self, session: &SessionRef) -> Option<HistoryProbe> {
        crate::tmux::pane_log_probe(self.sock(), session.as_str()).map(|p| HistoryProbe {
            history: p.history,
            limit: p.limit,
            bytes: p.bytes,
            alternate: p.alternate,
        })
    }

    fn history_probe_batch(&self) -> Vec<(SessionRef, HistoryProbe)> {
        crate::tmux::pane_log_probe_batch(self.sock())
            .into_iter()
            .filter_map(|(name, p)| {
                Some((
                    SessionRef::new(name).ok()?,
                    HistoryProbe {
                        history: p.history,
                        limit: p.limit,
                        bytes: p.bytes,
                        alternate: p.alternate,
                    },
                ))
            })
            .collect()
    }

    fn send_text(&self, session: &SessionRef, text: &str) -> Result<(), BackendError> {
        crate::tmux::send_keys(self.sock(), session.as_str(), text).map_err(BackendError::Operation)
    }

    fn send_key(&self, session: &SessionRef, key: &str) -> Result<(), BackendError> {
        crate::tmux::send_key(self.sock(), session.as_str(), key).map_err(BackendError::Operation)
    }

    fn paste(&self, session: &SessionRef, text: &str) -> Result<(), BackendError> {
        crate::tmux::paste_text(self.sock(), session.as_str(), text)
            .map_err(BackendError::Operation)
    }

    fn resize_window(
        &self,
        session: &SessionRef,
        window: u32,
        cols: u32,
        rows: u32,
    ) -> Result<(), BackendError> {
        crate::tmux::resize_window(self.sock(), session.as_str(), window, cols, rows)
            .map_err(BackendError::Operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::SpawnCommand;
    use std::path::PathBuf;

    fn backend() -> TmuxBackend {
        TmuxBackend {
            socket: "tako-unit".into(),
        }
    }

    #[test]
    fn 能力はtmuxの器と到達の両方を申告する() {
        let caps = backend().capabilities();
        assert!(caps.survives_app_exit);
        assert!(caps.detached_access);
        assert_eq!(caps.scrollback, ScrollbackAuthority::Backend);
        assert_eq!(caps.label, "tmux");
        assert!(backend().detached().is_some());
    }

    /// **段取り ① の挙動不変の証拠**（設計 §8.2 の R1）。
    /// `wrap_spawn` が生成する argv を語単位で固定する。tmux コマンド層を
    /// 物理移設する段取り ③ で順序・有無が変われば、このテストが落ちる。
    ///
    /// 固定している不変条件（いずれも実機バグの再発防止）:
    /// - `-u`: Finder 起動の .app はロケール環境変数が無く、無いと CJK が `_` に潰れる
    /// - `-f <conf>`: ユーザーの `~/.tmux.conf` を読まない（裏方に徹する）
    /// - `new-session -A -D`: 新規作成と再 attach が同一コマンドになる（FR-5 の復元）
    /// - `-e TAKO_PANE_ID` / `-e TAKO_TAB_ID`: サーバーのグローバル環境の stale 値を
    ///   使わせない（#0156b9a の stale ID 問題）。**この 2 つ以外の env は `-e` に載せない**
    #[test]
    fn wrap_spawn_argv_スナップショット() {
        let b = backend();
        let session = SessionRef::new("tako-0123456789ab").unwrap();
        let options = SpawnOptions {
            command: None,
            cwd: Some(PathBuf::from("/tmp/work")),
            env: vec![
                ("TAKO_PANE_ID".into(), "7".into()),
                ("TAKO_TOKEN".into(), "secret".into()),
                ("TAKO_TAB_ID".into(), "3".into()),
            ],
        };
        let wrapped = b.wrap_spawn(options.clone(), &session);
        let cmd = wrapped
            .command
            .expect("tmux クライアントの起動コマンドが要る");
        assert_eq!(cmd.program, crate::tmux::tmux_bin());

        // conf のパスは data_dir 依存なので位置と前後関係だけ固定する
        let conf_at = cmd
            .args
            .iter()
            .position(|a| a == "-f")
            .expect("-f が要る（ユーザー conf を読まない担保）");
        let mut args = cmd.args.clone();
        args.splice(
            conf_at..=conf_at + 1,
            ["-f".to_string(), "<conf>".to_string()],
        );

        assert_eq!(
            args,
            vec![
                "-u",
                "-L",
                "tako-unit",
                "-f",
                "<conf>",
                "new-session",
                "-A",
                "-D",
                "-s",
                "tako-0123456789ab",
                "-e",
                "TAKO_PANE_ID=7",
                "-e",
                "TAKO_TAB_ID=3",
                "-c",
                "/tmp/work",
            ]
        );

        // env / cwd は SpawnOptions 側にも残る（クライアント経由でセッション環境になる）
        assert_eq!(wrapped.env, options.env);
        assert_eq!(wrapped.cwd, options.cwd);
    }

    /// 明示コマンドは**クォートして 1 引数**で渡す（残余引数の空白連結 + `sh -c` 対策）。
    /// 未指定時はあえて渡さない（渡すと非対話 zsh ラッパーが ZDOTDIR を消費し、
    /// 内側の対話シェルにシェル統合が届かない。2026-06-12 のスパイクで判明）
    #[test]
    fn wrap_spawn_は明示コマンドを1引数へクォートする() {
        let b = backend();
        let session = SessionRef::new("tako-0123456789ab").unwrap();
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/zsh".into(),
                args: vec!["-lc".into(), "npm run dev".into()],
            }),
            cwd: None,
            env: vec![],
        };
        let args = b.wrap_spawn(options, &session).command.unwrap().args;
        let inner = args.last().unwrap();
        assert!(
            inner.contains("/bin/zsh"),
            "内側コマンドが 1 引数に畳まれている: {inner}"
        );
        assert!(
            inner.contains("npm run dev"),
            "空白を含む引数がクォートされている: {inner}"
        );
        assert!(
            !args.contains(&"-c".to_string()),
            "cwd 未指定なら -c を付けない"
        );
    }

    #[test]
    fn reserveはターゲット式の候補名を弾く() {
        let b = backend();
        assert!(b.reserve("tako-0123456789ab").is_some());
        assert!(b.reserve("tako-abc:0.0").is_none());
        assert!(b.reserve("").is_none());
    }

    #[test]
    fn foreign_holdersは空入力でtmuxを呼ばない() {
        // 復元対象が無いのに list-clients を叩かない（起動レイテンシの回帰防止）
        assert!(backend().foreign_holders(&[]).is_empty());
    }
}

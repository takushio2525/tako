//! NullBackend — 器を持たないバックエンド（Windows 初期リリース / tmux 不在の macOS）
//!
//! 設計 §2.3（案 C）の実装。**新しい劣化を作るのではなく、#30 で実装・検証済みの
//! 「tmux 不在 = 構造のみ永続化」経路を正式な選択肢として名前を付けたもの**である。
//! Homebrew 配布先（tmux 無し環境）はこの経路の本番実績にあたる。
//!
//! この backend での期待挙動:
//!
//! - **保存はする**（layout.json は書かれる）。保存を止めるのは #30 の根因 1 そのもの
//! - 復元はタブ / ペイン構成・cwd・プレビュー・Web ビュー・claude 会話まで。
//!   実行中プロセスと画面内容は tako 終了時に失われる
//! - `detached_capture()` / `detached()` がどちらも `None` = tako-app 不在では
//!   画面を読めず、キーも送れない
//! - スクロールは縮退しない。履歴は in-process の alacritty が持ち、
//!   直接ペイン経路（#159 の本命実装）がそのまま効く（`scrollback: InProcess`）

use std::collections::HashSet;
use std::time::Duration;

use super::{
    BackendCapabilities, BackendError, Holder, ScrollbackAuthority, SessionBackend, SessionInfo,
    SessionRef,
};
use crate::terminal::SpawnOptions;

pub struct NullBackend;

impl SessionBackend for NullBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            survives_app_exit: false,
            detached_capture: false,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            // 器が無い = シェルの出力が素のまま tako の PTY に届く
            osc_passthrough: true,
            label: "none",
        }
    }

    /// 器を配らない。呼び出し側はこれを見て「バックエンドセッション無し」の
    /// ペインとして扱う（`backend_sessions` に入らない = 直接ペイン経路）
    fn reserve(&self, _candidate: &str) -> Option<SessionRef> {
        None
    }

    /// 恒等変換。シェルはそのまま in-process の PTY で起動する
    fn wrap_spawn(&self, options: SpawnOptions, _session: &SessionRef) -> SpawnOptions {
        options
    }

    fn exists(&self, _session: &SessionRef) -> bool {
        false
    }

    fn kill(&self, _session: &SessionRef) -> Result<(), BackendError> {
        // 器が無ければ kill 対象も無い。エラーにはしない（呼び出し側の
        // close 経路が「セッションを消してから」を条件にしないため。#30 の CloseReason 参照）
        Ok(())
    }

    fn list(&self) -> Vec<SessionInfo> {
        Vec::new()
    }

    /// 常に空 = #177 の復元強奪ガードは発動しない。
    /// 器が無ければ「別インスタンスが表示中の資源」自体が存在しないため正しい
    fn foreign_holders(&self, _sessions: &[SessionRef]) -> Vec<Holder> {
        Vec::new()
    }

    /// 常に空 = #191 の orphan 自動復帰も FR-2.16.11 の cleanup も発動しない
    fn orphans(&self, _protected: &HashSet<SessionRef>) -> Vec<SessionRef> {
        Vec::new()
    }

    fn cleanup_orphans(
        &self,
        _protected: &HashSet<SessionRef>,
        _min_idle: Option<Duration>,
    ) -> Vec<SessionRef> {
        Vec::new()
    }

    /// 直接ペインの tty は `TerminalSession` 側が持つ（器の tty ではない）
    fn pane_tty(&self, _session: &SessionRef) -> Option<String> {
        None
    }

    /// 器が無ければ「器の cwd」も無い（復元 cwd は layout.json 側が持つ）
    fn session_cwd(&self, _session: &SessionRef) -> Option<String> {
        None
    }

    fn session_env(&self, _session: &SessionRef, _name: &str) -> Option<String> {
        None
    }

    fn set_session_env(&self, _session: &SessionRef, _name: &str, _value: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn opts() -> SpawnOptions {
        SpawnOptions {
            command: Some(crate::terminal::SpawnCommand {
                program: "/bin/zsh".into(),
                args: vec!["-l".into()],
            }),
            cwd: Some(PathBuf::from("/tmp")),
            env: vec![("TAKO_PANE_ID".into(), "7".into())],
        }
    }

    #[test]
    fn 器を配らず能力も申告しない() {
        let b = NullBackend;
        let caps = b.capabilities();
        assert!(!caps.survives_app_exit);
        assert!(!caps.detached_capture);
        assert!(!caps.detached_access);
        assert_eq!(caps.scrollback, ScrollbackAuthority::InProcess);
        assert!(!caps.full_restore());
        assert!(b.reserve("tako-0123456789ab").is_none());
        assert!(b.detached().is_none());
        assert!(b.detached_capture().is_none(), "採取の入口も持たない");
    }

    #[test]
    fn wrap_spawnは恒等変換で環境変数もcwdも保つ() {
        // 器が無くても TAKO_PANE_ID の注入・cwd 継承は in-process の PTY へ
        // そのまま渡る（orchestrator spawn の env 注入が縮退しない根拠）
        let b = NullBackend;
        let s = SessionRef::new("tako-0123456789ab").unwrap();
        let before = opts();
        let after = b.wrap_spawn(before.clone(), &s);
        let (bc, ac) = (before.command.unwrap(), after.command.unwrap());
        assert_eq!(ac.program, bc.program);
        assert_eq!(ac.args, bc.args);
        assert_eq!(after.cwd, before.cwd);
        assert_eq!(after.env, before.env);
    }

    #[test]
    fn 一覧系はすべて空でガードが発動しない() {
        let b = NullBackend;
        let s = SessionRef::new("tako-0123456789ab").unwrap();
        assert!(b.list().is_empty());
        assert!(b.foreign_holders(std::slice::from_ref(&s)).is_empty());
        assert!(b.orphans(&HashSet::new()).is_empty());
        assert!(b
            .cleanup_orphans(&HashSet::new(), Some(Duration::from_secs(3600)))
            .is_empty());
        assert!(!b.exists(&s));
        assert!(b.kill(&s).is_ok());
        assert!(b.pane_tty(&s).is_none());
        assert!(b.session_env(&s, "TAKO_PANE_ID").is_none());
    }
}

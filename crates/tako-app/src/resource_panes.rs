//! PC リソースの採取（`tako_control::resources`）へペインの一覧を渡す（FR-2.43 / Issue #1993）
//!
//! 採取そのもの（libproc / sysctl / `tmux list-panes -a`）は tako-control の背景スレッドが行い、
//! **UI スレッドはここで「どのペインがどの pid を根に持つか」を写して渡すだけ**（走査も子プロセスもしない。
//! 番犬 `crates/tako-control/tests/issue1993_resources_ui_thread_watchdog.rs`）。
//!
//! 根になる pid は、器の無いペインなら PTY 直下の子（シェル）、器のペインなら器のセッション名
//! （直下は attach の客なので、根は背景スレッドが `pane_pid` で引く）。

use tako_control::resources::PaneEntry;

use super::*;

impl TakoApp {
    /// 2 秒 tick（`periodic_prep`）から呼ぶ。タブの中のペインと、たまり場（退避中）のペインを渡す
    pub(crate) fn publish_resource_panes(&self) {
        let mut entries = Vec::with_capacity(self.terminals.len());
        for (&id, session) in &self.terminals {
            let backend_session = self.backend_sessions.get(&id).cloned();
            let child_pid = if backend_session.is_some() {
                None
            } else {
                session.child_pid()
            };
            let (tab_title, pane, shelved) = match self.workspace.find_tab_of_pane(id) {
                Some(tab_id) => {
                    let tab = self.workspace.get_tab(tab_id);
                    let pane =
                        tab.and_then(|t| t.tree().panes().into_iter().find(|p| p.id() == id));
                    (tab.map(|t| t.title().to_string()), pane, false)
                }
                None => match self.workspace.background_pane(id) {
                    Some((pane, _, origin_title)) => {
                        (Some(origin_title.to_string()), Some(pane), true)
                    }
                    None => (None, None, false),
                },
            };
            entries.push(PaneEntry {
                pane: id.as_u64(),
                tab_title,
                title: pane.and_then(|p| p.title()).map(str::to_string),
                role: pane.and_then(|p| p.role()).map(str::to_string),
                shelved,
                child_pid,
                backend_session,
            });
        }
        let own_socket = self.ipc.as_ref().map(|ipc| ipc.endpoint().to_string());
        tako_control::resources::publish_panes(entries, own_socket);
    }
}

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
                queue_priority: pane.and_then(|p| p.queue_priority()),
            });
        }
        let own_socket = self.ipc.as_ref().map(|ipc| ipc.endpoint().to_string());
        tako_control::resources::publish_panes(entries, own_socket);
    }

    /// 2 秒 tick（`periodic_prep`）から呼ぶ（#1997）。走査の覚えのペインごとの常駐メモリを写す
    /// （新しい走査が来たときだけ。**採らない**）。写した値が変わったら true（fleet を見ていれば描き直す）
    pub(crate) fn refresh_pane_resident(&mut self) -> bool {
        let Some((at, panes)) = tako_control::resources::pane_resident(self.pane_resident_at)
        else {
            return false;
        };
        self.pane_resident_at = Some(at);
        let next: HashMap<PaneId, tako_control::resources::PaneResident> = panes
            .into_iter()
            .map(|(pane, r)| (PaneId::from_raw(pane), r))
            .collect();
        let changed = next != self.pane_resident;
        self.pane_resident = next;
        changed
    }

    /// `tako panel` の応答の `fleet`（#1997）: fleet ビューの worker の行（描画と同じ `tmux_view_groups`）の
    /// ペイン番号と常駐メモリ・fleet を見ているか・写した走査の古さ
    pub(crate) fn fleet_state_json(&self) -> serde_json::Value {
        let workers: Vec<serde_json::Value> = self
            .tmux_view_groups()
            .iter()
            .flat_map(|g| g.rows.iter())
            .filter_map(|r| {
                r.resident_mb
                    .map(|mb| serde_json::json!({ "pane": r.pane.as_u64(), "resident_mb": mb }))
            })
            .collect();
        serde_json::json!({
            "shown": self.panel_visible && self.panel_view == PanelView::Fleet,
            "workers": workers,
            "resident_age_s": self.pane_resident_at.map(|at| at.elapsed().as_secs()),
        })
    }

    /// fleet ビューの worker の行に出す常駐メモリ（MiB・子孫込み。#1997）。worker でないペイン・走査に
    /// 載っていないペインは `None`。`workers` の応答の `resident.memory_mb` と同じ走査の値
    pub(crate) fn worker_resident_mb(&self, pane: PaneId, role: Option<&str>) -> Option<u64> {
        role.filter(|r| r.starts_with("orchestrator-worker"))?;
        self.pane_resident.get(&pane).map(|r| r.memory_mb)
    }
}

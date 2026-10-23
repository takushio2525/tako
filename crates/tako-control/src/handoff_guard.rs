//! handoff_guard — 引き継ぎの帳面と、dispatch の close が引く守り（Issue #2330）
//!
//! 判断（どの close を断るか）は `tako_core::handoff_guard::decide` の 1 実装。
//! ここは帳面（[`HandoffBook`]）を GUI プロセスに 1 冊だけ置き、workspace から判断の材料を
//! 組むだけ。帳面へ書くのは 3 か所:
//!
//! - 引き継ぎの発行（`dispatch_orchestrator_handoff`）→ [`note_issued`]
//! - 送達フローの決着（tako-app。`delivered` になった送達）→ [`note_delivered`]
//! - ペインを閉じた（dispatch の close・画面からの close）→ [`forget`]

use std::sync::{Mutex, OnceLock};

use tako_core::handoff_guard::{self as core, CloseQuery, Handoff, HandoffBook, Refusal};
use tako_core::{PaneId, TabId, Workspace};

fn book() -> &'static Mutex<HandoffBook> {
    static BOOK: OnceLock<Mutex<HandoffBook>> = OnceLock::new();
    BOOK.get_or_init(Default::default)
}

/// 引き継ぎを発行した（後任を立てて引き継ぎ文を積んだ）
pub fn note_issued(successor: u64, predecessor: u64) {
    if let Ok(mut b) = book().lock() {
        b.issue(successor, predecessor);
    }
}

/// そのペインへの送達が `delivered` で決着した（顛末コード = 受領の証拠）。
/// 引き継ぎの後任なら受領済みにして persist.log へ 1 行残す（本文は書かない）。
///
/// **届いたと言える顛末だけ**を受領にする（`verified` / `received` / peer の `delivered`）。
/// 「送ったかもしれない」（`peer_unconfirmed` 等。状態は delivered でも確からしさは unverified）は
/// 受領にしない = 前任が閉じ直す道を残す。判定の正本は `prompt_delivery::outcome_confidence`
pub fn note_delivered(pane: u64, outcome: &str) {
    use tako_core::prompt_delivery::{outcome_confidence, Confidence};
    if outcome_confidence(outcome) != Confidence::Delivered {
        return;
    }
    let Ok(mut b) = book().lock() else {
        return;
    };
    if b.note_received(pane, outcome) {
        let predecessor = b.get(pane).map(|h| h.predecessor).unwrap_or_default();
        drop(b);
        crate::diag::persist_log(&format!(
            "引き継ぎ: 後任が引き継ぎ文を受け取った（後任={pane} 前任={predecessor} 顛末={outcome}）。\
             以後、前任からは後任を閉じない（#2330）"
        ));
    }
}

/// ペインが閉じられた（後任としての記録も、前任としての記録 = 引き継ぎの完了も消す）
pub fn forget(pane: u64) {
    if let Ok(mut b) = book().lock() {
        b.forget(pane);
    }
}

/// 後任のペインから引き継ぎを引く（写し）
pub fn handoff_of(successor: u64) -> Option<Handoff> {
    book().lock().ok()?.get(successor).cloned()
}

/// close の守り。対象のタブ・呼び出し元・帳面から材料を組んで [`core::decide`] に問う。
///
/// `caller_pane` は MCP の呼び出し元（`TAKO_PANE_ID` / `X-Tako-Pane`）か CLI の `TAKO_PANE_ID`。
/// `TAKO_2330_LEGACY=1` では常に通す（修正前）
pub fn check_close(
    ws: &Workspace,
    tab: TabId,
    target: PaneId,
    caller_pane: Option<u64>,
    caller_role: Option<&str>,
) -> Result<(), Refusal> {
    if core::legacy() {
        return Ok(());
    }
    let Some(tree) = ws.get_tab(tab).map(|t| t.tree()) else {
        return Ok(());
    };
    let target_role = tree.get(target).and_then(|p| p.role());
    let tab_roles: Vec<Option<&str>> = tree
        .panes()
        .into_iter()
        .filter(|p| p.id() != target)
        .map(|p| p.role())
        .collect();
    let caller_pane_role = caller_pane
        .and_then(|id| ws.pane_anywhere(PaneId::from_raw(id)))
        .and_then(|p| p.role());
    // 受領済みの後任か。前任がまだ生きている（master のまま）ときだけ守る
    // （前任が閉じられた後は引き継ぎが終わっている = 最後の master の守りだけが残る）
    let handoff = handoff_of(target.as_u64());
    let received_from = handoff.as_ref().and_then(|h| {
        let evidence = h.received.as_deref()?;
        let alive = ws
            .pane_anywhere(PaneId::from_raw(h.predecessor))
            .and_then(|p| p.role())
            .is_some_and(|r| tako_core::handoff::master_profile_of_any_role(r).is_some());
        alive.then_some((h.predecessor, evidence))
    });
    core::decide(&CloseQuery {
        target: target.as_u64(),
        target_role,
        tab_roles,
        caller_pane,
        caller_role,
        caller_pane_role,
        received_from,
    })
}

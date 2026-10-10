//! リミット後の自動復帰の一括切り替えと全体の既定（Issue #1945）。
//!
//! #813 の自動復帰は**ペイン単位**のオプトインで、既定 OFF。全部の master・worker・
//! ソロ・手で起動した claude を ON にするには 1 本ずつ `--pane N` を打つしかなく、
//! 退避中のペインは指定すらできず、新しく立つペインは毎回 OFF から始まった。
//!
//! ここは「どのペインが一括の対象か」「いまの全体の状態はどれか」「一括で何が
//! 変わるか」「以後に立つペインへ既定をどう当てるか」の**判断だけ**を持つ純粋な層。
//! ステータスバーのボタン・CLI・MCP はどれも dispatch の `LimitResume` を通り、
//! dispatch と GUI の両方がここを呼ぶ（UI 層に判断を置かない）。
//!
//! # 「エージェントのペイン」
//!
//! role が master / worker / solo のペイン（tako が立てた）か、GUI の会話検出
//! （`claude_resume` / `agent_resume`）に載っているペイン（手で起動した claude /
//! codex / agy）。シェル・プレビュー・実行ペインは対象外 —— 自動復帰の継続ナッジは
//! エージェントの入力欄に打つ前提なので、シェルに打つと**コマンドとして実行される**。
//!
//! # 全体の既定と「決定済み」
//!
//! 一括操作は全体の既定（settings.json の `limit_resume_all`）も書き換える。
//! 以後エージェントになったペインは、まだ値が決まっていなければ既定を採る
//! （[`crate::Pane::adopt_limit_resume_default`]）。人が個別に切ったペインは
//! 「決定済み」なので、次の一括操作までは全体の既定で上書きしない。

use crate::{Pane, PaneId, Workspace};

/// A/B（#1945 前の挙動）: 一括は一覧だけ・退避中は指定できない・既定を配らない・
/// 自動復帰の駆動は表示中のタブだけ
pub fn legacy() -> bool {
    std::env::var("TAKO_1945_LEGACY").is_ok_and(|v| v == "1")
}

/// role から見てエージェントのペインか（master / worker / solo）
pub fn is_agent_role(role: &str) -> bool {
    crate::workspace::is_master_role(role)
        || crate::workspace::is_worker_role(role)
        || role == "solo"
        || role.starts_with("solo:")
}

/// 一括の対象（エージェントのペイン）か。`detected` は GUI の会話検出に載っているか
/// （GUI 以外のホストは常に false = role だけで判断する）
pub fn is_agent_pane(pane: &Pane, detected: bool) -> bool {
    detected || pane.role().is_some_and(is_agent_role)
}

/// 全体の状態（ステータスバーのボタンの見た目と、押したときの向きを決める）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkState {
    /// エージェントのペインが全部 ON で、以後に立つペインも ON
    AllOn,
    /// ペインごとに揃っていない、または既定とペインが食い違っている
    Partial,
    /// エージェントのペインが全部 OFF で、以後に立つペインも OFF
    AllOff,
}

impl BulkState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllOn => "all_on",
            Self::Partial => "partial",
            Self::AllOff => "all_off",
        }
    }
}

/// エージェントのペインの集計
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BulkSummary {
    /// 自動復帰が ON のエージェントのペイン数
    pub on: usize,
    /// エージェントのペイン数（表示中 + 退避中）
    pub total: usize,
    /// 以後に立つエージェントのペインの既定（全体の既定）
    pub default: bool,
}

impl BulkSummary {
    /// 全体の状態。**既定も見る**: ペインが全部 ON でも以後に立つペインが OFF なら
    /// 「全部」とは言わない（ボタンが ON と言っているのに新しい master が OFF で立つ、を防ぐ）
    pub fn state(&self) -> BulkState {
        if self.default && self.on == self.total {
            BulkState::AllOn
        } else if !self.default && self.on == 0 {
            BulkState::AllOff
        } else {
            BulkState::Partial
        }
    }

    /// ボタンを押したときに揃える値。全部 ON なら OFF、それ以外（一部・全部 OFF）は ON
    /// （一部のときに OFF へ倒すと、押した人が ON にしたかったペインまで落ちる）
    pub fn next_enabled(&self) -> bool {
        self.state() != BulkState::AllOn
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state().as_str(),
            "on": self.on,
            "total": self.total,
            "default": self.default,
        })
    }
}

/// 全ペイン（表示中 + 退避中）を集計する。`detected` は GUI の会話検出
pub fn summarize(ws: &Workspace, default: bool, detected: impl Fn(PaneId) -> bool) -> BulkSummary {
    let mut summary = BulkSummary {
        default,
        ..BulkSummary::default()
    };
    for pane in ws.all_panes() {
        if !is_agent_pane(pane, detected(pane.id())) {
            continue;
        }
        summary.total += 1;
        if pane.limit_autoresume() {
            summary.on += 1;
        }
    }
    summary
}

/// 一括で揃える。値が変わったペインの ID を返す（表示中 → 退避中の順）。
///
/// 対象は**エージェントのペインすべて**（退避中を含む）。決定済みかどうかは見ない
/// （一括は人の明示の操作なので、個別に切ったペインも揃える）
pub fn apply_bulk(
    ws: &mut Workspace,
    enabled: bool,
    detected: impl Fn(PaneId) -> bool,
) -> Vec<PaneId> {
    let targets: Vec<PaneId> = ws
        .all_panes()
        .into_iter()
        .filter(|p| is_agent_pane(p, detected(p.id())))
        .map(|p| p.id())
        .collect();
    let mut changed = Vec::new();
    for id in targets {
        let Some(pane) = ws.pane_anywhere_mut(id) else {
            continue;
        };
        let before = pane.limit_autoresume();
        pane.set_limit_autoresume(enabled);
        if before != enabled {
            changed.push(id);
        }
    }
    changed
}

/// エージェントになったペインへ全体の既定を当てる（以後に立つペイン）。
/// 値が変わった（ON になった）ペインの ID を返す。
///
/// 呼ぶのは「ペインがエージェントになったと分かった時点」= GUI の会話検出の直後。
/// role を貼る経路（master / solo / worker）は dispatch がその場で当てる
pub fn adopt_default(
    ws: &mut Workspace,
    default: bool,
    detected: impl Fn(PaneId) -> bool,
) -> Vec<PaneId> {
    if !default {
        return Vec::new();
    }
    let targets: Vec<PaneId> = ws
        .all_panes()
        .into_iter()
        .filter(|p| !p.limit_resume_decided() && is_agent_pane(p, detected(p.id())))
        .map(|p| p.id())
        .collect();
    targets
        .into_iter()
        .filter(|id| {
            ws.pane_anywhere_mut(*id)
                .is_some_and(|p| p.adopt_limit_resume_default(default))
        })
        .collect()
}

/// 復元直後のエージェントのペインを「決定済み」にする。
///
/// 保存されていた値は人の選択（全体を ON にした後で個別に OFF にした、など）なので、
/// 再起動で全体の既定に戻さない。**エージェントでなかったペイン**（シェル）は
/// 決定済みにしない = 再起動後にそこで claude を起動したら全体の既定を採る
pub fn mark_restored_agents_decided(ws: &mut Workspace, detected: impl Fn(PaneId) -> bool) {
    let targets: Vec<PaneId> = ws
        .all_panes()
        .into_iter()
        .filter(|p| is_agent_pane(p, detected(p.id())))
        .map(|p| p.id())
        .collect();
    for id in targets {
        if let Some(pane) = ws.pane_anywhere_mut(id) {
            pane.mark_limit_resume_decided();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PaneOrigin, SplitDirection};

    /// 表示中に master / shell / 手動 claude（検出のみ）、退避中に worker を置いた作業場
    fn workspace() -> (Workspace, [PaneId; 4]) {
        let mut master = Pane::new(PaneOrigin::User);
        master.set_role(Some("orchestrator-master".into()));
        let master_id = master.id();
        let mut ws = Workspace::new("1", master);
        let shell = Pane::new(PaneOrigin::User);
        let shell_id = shell.id();
        let manual = Pane::new(PaneOrigin::User);
        let manual_id = manual.id();
        let mut worker = Pane::new(PaneOrigin::Mcp);
        worker.set_role(Some("orchestrator-worker:tako".into()));
        let worker_id = worker.id();
        let tree = ws.active_tab_mut().tree_mut();
        tree.split(master_id, SplitDirection::Right, shell).unwrap();
        tree.split(shell_id, SplitDirection::Down, manual).unwrap();
        tree.split(manual_id, SplitDirection::Down, worker).unwrap();
        ws.shelve_pane(worker_id).unwrap();
        (ws, [master_id, shell_id, manual_id, worker_id])
    }

    #[test]
    fn エージェントのroleはmasterとworkerとsolo() {
        for role in [
            "orchestrator-master",
            "orchestrator-master:docs",
            "master",
            "master:docs",
            "orchestrator-worker:tako",
            "worker-1",
            "solo",
            "solo:docs",
        ] {
            assert!(is_agent_role(role), "{role}");
        }
        for role in ["", "dev-server", "soloist", "preview"] {
            assert!(!is_agent_role(role), "{role}");
        }
    }

    /// 一括は退避中を含むエージェントのペインだけを揃え、シェルは触らない
    #[test]
    fn 一括は退避中を含むエージェントだけを揃える() {
        let (mut ws, [master, shell, manual, worker]) = workspace();
        let detected = |id: PaneId| id == manual;
        let changed = apply_bulk(&mut ws, true, detected);
        assert_eq!(changed, vec![master, manual, worker]);
        assert!(ws.pane_anywhere(worker).unwrap().limit_autoresume());
        assert!(!ws.pane_anywhere(shell).unwrap().limit_autoresume());
        let s = summarize(&ws, true, detected);
        assert_eq!((s.on, s.total, s.state()), (3, 3, BulkState::AllOn));
        // 2 回目は何も変わらない（冪等）
        assert!(apply_bulk(&mut ws, true, detected).is_empty());
        let changed = apply_bulk(&mut ws, false, detected);
        assert_eq!(changed, vec![master, manual, worker]);
        let s = summarize(&ws, false, detected);
        assert_eq!((s.on, s.total, s.state()), (0, 3, BulkState::AllOff));
    }

    /// 状態は既定も見る。押したときの向きは「全部 ON だけ OFF へ」
    #[test]
    fn 状態と押したときの向き() {
        let s = |on, total, default| BulkSummary { on, total, default };
        assert_eq!(s(3, 3, true).state(), BulkState::AllOn);
        assert_eq!(s(0, 3, false).state(), BulkState::AllOff);
        assert_eq!(s(1, 3, true).state(), BulkState::Partial);
        assert_eq!(s(1, 3, false).state(), BulkState::Partial);
        // ペインは全部 ON でも以後が OFF なら「全部」ではない
        assert_eq!(s(3, 3, false).state(), BulkState::Partial);
        assert_eq!(s(0, 3, true).state(), BulkState::Partial);
        // エージェントが 0 本なら既定だけで決まる
        assert_eq!(s(0, 0, true).state(), BulkState::AllOn);
        assert_eq!(s(0, 0, false).state(), BulkState::AllOff);
        assert!(!s(3, 3, true).next_enabled());
        assert!(s(1, 3, true).next_enabled());
        assert!(s(0, 3, false).next_enabled());
        assert!(s(0, 0, false).next_enabled());
        assert!(!s(0, 0, true).next_enabled());
    }

    /// 以後にエージェントになったペインは既定を採る。個別に切ったペインは上書きしない
    #[test]
    fn 既定は決まっていないペインにだけ当たる() {
        let (mut ws, [master, _shell, manual, worker]) = workspace();
        // 既定 OFF では何も起きず、印も立たない（後で ON にしたときに採れる）
        assert!(adopt_default(&mut ws, false, |id| id == manual).is_empty());
        assert!(!ws.pane_anywhere(manual).unwrap().limit_resume_decided());
        // 人が master を個別に OFF にした（決定済み）
        ws.pane_anywhere_mut(master)
            .unwrap()
            .set_limit_autoresume(false);
        let adopted = adopt_default(&mut ws, true, |id| id == manual);
        assert_eq!(adopted, vec![manual, worker], "退避中の worker も採る");
        assert!(!ws.pane_anywhere(master).unwrap().limit_autoresume());
        // 2 回目は何も起きない（決定済みになった）
        assert!(adopt_default(&mut ws, true, |id| id == manual).is_empty());
        // 採った後に人が OFF にしたら、次の検出でも戻さない
        ws.pane_anywhere_mut(manual)
            .unwrap()
            .set_limit_autoresume(false);
        assert!(adopt_default(&mut ws, true, |id| id == manual).is_empty());
        assert!(!ws.pane_anywhere(manual).unwrap().limit_autoresume());
    }

    /// 復元したエージェントのペインは保存時の値を保ち、シェルは後から既定を採れる
    #[test]
    fn 復元したエージェントは決定済みでシェルは未決定() {
        let (mut ws, [master, shell, manual, worker]) = workspace();
        mark_restored_agents_decided(&mut ws, |id| id == manual);
        for id in [master, manual, worker] {
            assert!(ws.pane_anywhere(id).unwrap().limit_resume_decided(), "{id}");
        }
        assert!(!ws.pane_anywhere(shell).unwrap().limit_resume_decided());
        // 復元後にシェルで claude を起動した = 検出に載った
        let adopted = adopt_default(&mut ws, true, |id| id == manual || id == shell);
        assert_eq!(adopted, vec![shell]);
    }

    /// 退避中のペインも ID で引け、書き換えられる（#1945 前は「見つからない」）
    #[test]
    fn 退避中のペインもidで引ける() {
        let (mut ws, [_, _, _, worker]) = workspace();
        assert!(ws.is_shelved(worker));
        assert!(ws.pane_anywhere(worker).is_some());
        ws.pane_anywhere_mut(worker)
            .unwrap()
            .set_limit_autoresume(true);
        assert!(ws.pane_anywhere(worker).unwrap().limit_autoresume());
        assert!(ws.pane_anywhere(PaneId::from_raw(u64::MAX)).is_none());
    }
}

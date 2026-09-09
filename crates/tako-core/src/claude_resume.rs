//! claude_resume — 復元用「ペイン → claude セッション ID」の保持規則（Issue #1076）
//!
//! PC 再起動では tmux サーバーごと消えるので、ペインを起こし直すときに
//! `claude --resume <id>` を投げるしかない。その `<id>` は `layout.json` に
//! 保存してあるが、**保存済み ID の落とし方**が壊れていた（= #1076 の根因）。
//!
//! # なぜ丸ごと置き換えてはいけないか
//!
//! ID の出どころは `claude agents --json` の定期スキャン（既定 30 秒間隔・
//! 起動から 5 秒後に 1 回目）で、これは**「いま動いているか」しか答えない**。
//! 旧実装はスキャン結果でマップを丸ごと置き換えていたため、
//!
//! - 再起動直後の 1 回目のスキャンは、resume した claude がまだ起動途中
//!   （Node の起動 + 巨大な transcript の読み込みで数十秒かかる）なので
//!   **1 件も検出できない**
//! - その結果マップが空になり、`save_layout` が `layout.json` から
//!   `claude_session_id` を**全部消す**
//!
//! という順序で、起動から数秒後に復元の種を自分で捨てていた
//! （実測 2026-09-09: 復元直後 12 件 → 2 秒後 0 件）。一度捨てると次の
//! 再起動では「タブのタイトルだけ残って claude が居ない」= ユーザー報告の症状になる。
//!
//! # 規約: 確認してから外す（confirm-then-forget）
//!
//! **不在は「終了した」の証拠にならない**（起動途中・スキャンの取りこぼし・
//! 承諾ダイアログで待っている claude はどれも検出に出ない）。そこで
//!
//! - [`ResumeIds::seed`]（`layout.json` 由来）で入った ID は **未確認**として持ち、
//!   スキャンの不在では**絶対に落とさない**
//! - スキャンで一度でも検出できたペイン（= 確認済み）だけが、
//!   連続 [`FORGET_AFTER_MISSES`] 回の不在で落ちる（ユーザーが claude を終了して
//!   シェルに戻したペインを次回起動で勝手に resume しない、という旧来の意図は保つ）
//! - 生きているペインの一覧から消えたペイン（閉じた）は即座に落ちる
//!
//! 「確認できていない ID を残す」側に倒すと、最悪ケースは
//! 「ユーザーが閉じた会話が次回起動で 1 度だけ復活する」で済む。逆に倒すと
//! **会話への唯一の入口を失う**ので、非対称なコストに合わせて倒す向きを決めてある。

use std::collections::HashMap;

use crate::agent_resume::AgentResumeIds;
use crate::agent_support::Agent;
use crate::PaneId;

/// 確認済みのペインの ID を落とすまでに必要な連続不在回数。
/// 1 回の取りこぼし（スキャンが Node 起動に失敗した等）で落とさないための猶予。
/// **規則の正本は [`crate::agent_resume`]**（#1238 で claude 以外へも広げた）
pub use crate::agent_resume::FORGET_AFTER_MISSES;

/// #1076 の A/B。`TAKO_1076_LEGACY=1` で修正前の挙動
/// （検出結果でマップを丸ごと置き換える + 起動条件を落とした最小形の resume）へ戻す
pub fn legacy_1076() -> bool {
    std::env::var_os("TAKO_1076_LEGACY").is_some()
}

/// ペインごとの復元用 claude セッション ID（[`crate::claude_resume`] の規約を実装する）。
///
/// **中身は [`AgentResumeIds`] の claude 固定の view**（#1238）。
/// 保持規則（確認してから外す）の実装を 2 つに割らないため、
/// ここはコンテナを持たず委譲だけを行う
#[derive(Debug, Clone, Default)]
pub struct ResumeIds {
    inner: AgentResumeIds,
}

impl ResumeIds {
    pub fn new() -> Self {
        Self::default()
    }

    /// `layout.json` から復元した ID を**未確認**として入れる。
    /// すでに確認済みの ID があるペインは触らない（検出結果のほうが新しい）
    pub fn seed(&mut self, pane: PaneId, id: &str) {
        self.inner.seed(pane, Agent::Claude, Some(id));
    }

    /// スキャン 1 回ぶんの結果を反映する。
    ///
    /// - `panes`: いま生きているペインの一覧（ここに無いペインの記録は捨てる）
    /// - `detected`: そのスキャンで claude を検出できたペインとその session ID
    ///
    /// **スキャンが失敗した回は呼ばない**（呼ぶと全ペインが不在扱いになる）。
    /// 「検出できたペインが 0 件」は呼んでよい（それが不在の 1 回になる）
    pub fn apply_scan(&mut self, panes: &[PaneId], detected: &HashMap<PaneId, String>) {
        self.inner.apply_scan(panes, &tag_claude(detected));
    }

    /// **#1076 の A/B 専用**（`TAKO_1076_LEGACY=1`）。修正前の
    /// 「検出結果でマップを丸ごと置き換える」を再現する。
    /// 製品経路では使わない（使うと #1076 が再発する）
    pub fn replace_all_legacy(&mut self, detected: &HashMap<PaneId, String>) {
        self.inner.replace_all_legacy(&tag_claude(detected));
    }

    /// ペインを閉じたときに記録を外す
    pub fn remove(&mut self, pane: PaneId) {
        self.inner.remove(pane);
    }

    /// そのペインの復元用 session ID
    pub fn get(&self, pane: PaneId) -> Option<&str> {
        self.inner.get(pane).and_then(|(_, id)| id)
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 確認済み（スキャンで実際に見えた）ペインの数。診断ログ用
    pub fn confirmed_count(&self) -> usize {
        self.inner.confirmed_count()
    }
}

/// 検出結果へ claude の札を付ける（このマップは claude 専用なので一律）
fn tag_claude(detected: &HashMap<PaneId, String>) -> HashMap<PaneId, (Agent, Option<String>)> {
    detected
        .iter()
        .map(|(pane, id)| (*pane, (Agent::Claude, Some(id.clone()))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(n: u64) -> PaneId {
        PaneId::from_raw(n)
    }

    fn detected(pairs: &[(u64, &str)]) -> HashMap<PaneId, String> {
        pairs
            .iter()
            .map(|(p, id)| (pane(*p), (*id).to_string()))
            .collect()
    }

    /// #1076 の根因: 復元した ID が「起動途中で検出できない」だけで消えていた
    #[test]
    fn 復元した未確認idはスキャンの不在で落ちない() {
        let mut ids = ResumeIds::new();
        ids.seed(pane(1), "aaa");
        ids.seed(pane(2), "bbb");
        // 再起動直後は claude がまだ起動途中で 1 件も検出できない
        for _ in 0..10 {
            ids.apply_scan(&[pane(1), pane(2)], &detected(&[]));
        }
        assert_eq!(ids.get(pane(1)), Some("aaa"));
        assert_eq!(ids.get(pane(2)), Some("bbb"));
        assert_eq!(ids.confirmed_count(), 0);
    }

    #[test]
    fn 検出できたらその値で確認済みになる() {
        let mut ids = ResumeIds::new();
        ids.seed(pane(1), "old");
        ids.apply_scan(&[pane(1)], &detected(&[(1, "new")]));
        assert_eq!(ids.get(pane(1)), Some("new"));
        assert_eq!(ids.confirmed_count(), 1);
    }

    /// 旧来の意図（ユーザーが claude を終了したペインを勝手に resume しない）は保つ
    #[test]
    fn 確認済みidは連続不在で落ちる() {
        let mut ids = ResumeIds::new();
        ids.apply_scan(&[pane(1)], &detected(&[(1, "aaa")]));
        assert_eq!(ids.get(pane(1)), Some("aaa"));
        // 1 回の取りこぼしでは落とさない
        ids.apply_scan(&[pane(1)], &detected(&[]));
        assert_eq!(ids.get(pane(1)), Some("aaa"), "1 回目の不在で落ちている");
        ids.apply_scan(&[pane(1)], &detected(&[]));
        assert_eq!(ids.get(pane(1)), None, "連続不在でも落ちない");
    }

    #[test]
    fn 不在が続いても間に検出が挟まればやり直しになる() {
        let mut ids = ResumeIds::new();
        ids.apply_scan(&[pane(1)], &detected(&[(1, "aaa")]));
        ids.apply_scan(&[pane(1)], &detected(&[]));
        ids.apply_scan(&[pane(1)], &detected(&[(1, "aaa")]));
        ids.apply_scan(&[pane(1)], &detected(&[]));
        assert_eq!(ids.get(pane(1)), Some("aaa"));
    }

    #[test]
    fn 生きていないペインの記録は捨てる() {
        let mut ids = ResumeIds::new();
        ids.seed(pane(1), "aaa");
        ids.seed(pane(2), "bbb");
        ids.apply_scan(&[pane(1)], &detected(&[]));
        assert_eq!(ids.get(pane(1)), Some("aaa"));
        assert_eq!(ids.get(pane(2)), None, "閉じたペインの記録が残っている");
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn closeで即座に外れる() {
        let mut ids = ResumeIds::new();
        ids.seed(pane(1), "aaa");
        ids.remove(pane(1));
        assert!(ids.is_empty());
    }

    /// seed は確認済みの値を上書きしない（復元は起動時 1 回だが、
    /// orphan 復元（#191）等で後から呼ばれても検出結果を巻き戻さない）
    #[test]
    fn seedは確認済みidを巻き戻さない() {
        let mut ids = ResumeIds::new();
        ids.apply_scan(&[pane(1)], &detected(&[(1, "new")]));
        ids.seed(pane(1), "old");
        assert_eq!(ids.get(pane(1)), Some("new"));
    }
}

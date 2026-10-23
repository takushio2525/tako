//! handoff_guard — master のペインを閉じる close の守り（Issue #2330）
//!
//! 2026-10-18、引き継ぎの前任と後任が 0.2 秒差で互いのペインを `force: true` で閉じ合い、
//! タブから master が消えた（約 2 日、誰も気付かなかった）。重なった原因は 2 つ:
//!
//! 1. 送達の偽陰性: 後任は引き継ぎ文を受け取っていたのに送達フローが `gave_up` を返し、
//!    前任は「届いていない」と読んで後任を閉じ直した（tako-app の送達フローと
//!    `tako_control::delivery_receipt` が受け手の会話記録を見て直す）
//! 2. 守りが無い: 引き継ぎ文は後任に「前任を閉じよ」と指示し、前任には「届かなければ閉じて
//!    やり直す」動機がある。どちらも `force` で master のペインを閉じられ、**タブの最後の
//!    master を消す close** を止めるものが無かった
//!
//! このモジュールは 2 の判断（純粋関数）と、引き継ぎの状態の帳面（[`HandoffBook`]）を持つ。
//! 帳面の実体は GUI プロセスに 1 冊（`tako_control::handoff_guard`）。
//!
//! # 守るもの（AI / CLI の close だけ。force でも断る）
//!
//! - **受領済みの後任**: 後任が引き継ぎ文を受け取った（送達が `delivered`）あとは、前任が
//!   生きているあいだ、ほかの master からその後任を閉じない。後任は手順どおり前任を閉じるので、
//!   前任 → 後任と後任 → 前任の 2 本のうち通るのは後者だけになる
//! - **最後の master**: 閉じると対象のタブにそのプロファイルの master が 1 人も残らない close。
//!   相互 close の 2 本目（先に閉じられた側の名乗りで残った側を閉じる）は必ずこれに当たる
//!
//! # 対象外（従来どおり通す）
//!
//! - 呼び出し元が master でない close（人がシェルで打つ CLI・スマホ・worker・role なし）
//! - master が自分のペインを閉じる close（自己片付け。master 同士の close ではない）
//! - **後任が自分の前任を閉じる close**（帳面に前任 → 後任があり、呼び出し元がその後任の生きた
//!   master のペイン）。タブの人数は見ない: 後任が別のタブに立った引き継ぎ（`--tab`・後から
//!   別タブへ動かした後任）では前任のタブに同じプロファイルの master が残らないが、後任自身が
//!   そのプロファイルの master として残る。断ると前任が幽霊として残る。受領は条件にしない
//!   （前任を閉じに来たこと自体が引き継ぎ文を読んだ証拠で、受領の検知が外れた系統で幽霊を作らない）。
//!   帳面が空（GUI の再起動後）なら従来どおり「最後の master」を見る
//! - 対象が master でない close
//! - 人が画面から閉じる経路（タブ / ペインの × ・⌘W）。dispatch を通らないので守りも掛からない。
//!   人の明示の操作であり、守りが AI を止めたときの逃げ道でもあるので止めない
//!
//! `TAKO_2330_LEGACY=1` で守りを外す（同じバイナリで修正前の挙動を再現する A/B の入口）。

use std::collections::HashMap;

use crate::handoff::master_profile_of_any_role;
use crate::i18n::Text;

/// `TAKO_2330_LEGACY=1` で #2330 前の挙動へ戻す（close の守りなし・送達は受領の証拠を見ない）
pub fn legacy() -> bool {
    std::env::var_os("TAKO_2330_LEGACY").is_some()
}

/// 引き継ぎ 1 件（後任の側から引く）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    /// 前任 master のペイン
    pub predecessor: u64,
    /// 後任が引き継ぎ文を受け取った証拠（送達の顛末コード = `verified` / `received` /
    /// `delivered`）。None = まだ確かめられていない（前任が閉じてやり直してよい段階）
    pub received: Option<String>,
}

/// 進行中の引き継ぎの帳面。**後任のペイン → 引き継ぎ**で引く。
///
/// 書くのは 3 か所だけ: 引き継ぎの発行（[`HandoffBook::issue`]）・送達フローの決着
/// （[`HandoffBook::note_received`]）・ペインを閉じたとき（[`HandoffBook::forget`]）。
/// GUI を再起動すると空になる（そのときは「最後の master」の守りだけが効く）
#[derive(Debug, Default, Clone)]
pub struct HandoffBook {
    by_successor: HashMap<u64, Handoff>,
}

impl HandoffBook {
    /// 引き継ぎを発行した（後任を立てて引き継ぎ文を積んだ）
    pub fn issue(&mut self, successor: u64, predecessor: u64) {
        self.by_successor.insert(
            successor,
            Handoff {
                predecessor,
                received: None,
            },
        );
    }

    /// そのペインへの送達が `delivered` で決着した。後任として帳面にあれば受領済みにして true
    /// （帳面に無いペイン = 引き継ぎと関係の無い送達は何もしない）
    pub fn note_received(&mut self, pane: u64, evidence: &str) -> bool {
        match self.by_successor.get_mut(&pane) {
            Some(h) if h.received.is_none() => {
                h.received = Some(evidence.to_string());
                true
            }
            _ => false,
        }
    }

    /// 後任のペインから引き継ぎを引く
    pub fn get(&self, successor: u64) -> Option<&Handoff> {
        self.by_successor.get(&successor)
    }

    /// ペインが閉じられた。後任としての記録も、前任としての記録（= 引き継ぎの完了）も消す
    pub fn forget(&mut self, pane: u64) {
        self.by_successor.remove(&pane);
        self.by_successor.retain(|_, h| h.predecessor != pane);
    }
}

/// close の可否を決める材料（dispatch が workspace と帳面から組む）
#[derive(Debug, Clone, Default)]
pub struct CloseQuery<'a> {
    /// 閉じる対象のペイン
    pub target: u64,
    /// 対象のペインの role（ペインのラベル）
    pub target_role: Option<&'a str>,
    /// 対象と同じタブにある**対象以外の**ペインの role
    pub tab_roles: Vec<Option<&'a str>>,
    /// 呼び出し元のペイン（MCP: `TAKO_PANE_ID` / `X-Tako-Pane`、CLI: `TAKO_PANE_ID`）。分からなければ None
    pub caller_pane: Option<u64>,
    /// 呼び出し元が名乗った role（`TAKO_ORCHESTRATOR_ROLE`。env 用の `master:<p>` でも
    /// ペインのラベルの `orchestrator-master:<p>` でもよい）
    pub caller_role: Option<&'a str>,
    /// 呼び出し元のペインに付いている role（名乗りを失った master も master と分かるように）
    pub caller_pane_role: Option<&'a str>,
    /// 対象が「受領済みの後任」なら、その前任（生きている master のペイン）と受領の証拠
    pub received_from: Option<(u64, &'a str)>,
    /// 帳面で**呼び出し元を後任とする**引き継ぎの前任（呼び出し元が誰の後任か）。
    /// 対象がこれと同じなら「後任が自分の前任を閉じる close」
    pub caller_predecessor: Option<u64>,
}

/// close を断る理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// 閉じるとタブにそのプロファイルの master が 1 人も残らない
    LastMaster { pane: u64, profile: String },
    /// 引き継ぎを受け取った後任（前任が生きている）
    ReceivedSuccessor {
        pane: u64,
        predecessor: u64,
        evidence: String,
    },
}

impl Refusal {
    /// 応答・ログ・テストが参照する安定コード
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::LastMaster { .. } => "last_master",
            Refusal::ReceivedSuccessor { .. } => "received_successor",
        }
    }

    /// 断った理由と次の一手（日英）。応答のエラー文になる
    pub fn note(&self) -> Text {
        match self {
            Refusal::LastMaster { pane, profile } => Text::new(
                format!(
                    "pane {pane} はこのタブで最後の {profile} の master なので閉じない\
                     （閉じるとタブから master が居なくなる。force でも同じ。#2330）。\
                     引き継ぎの途中なら、後任が前任を閉じるのを待つ。\
                     本当に閉じるならユーザーが画面から閉じる"
                ),
                format!(
                    "pane {pane} is the last {profile} master in this tab, so it is not closed \
                     (the tab would be left without a master; force does not override this. #2330). \
                     If a handoff is in progress, let the successor close the predecessor. \
                     If it really has to go, the user can close it from the UI"
                ),
            ),
            Refusal::ReceivedSuccessor {
                pane,
                predecessor,
                evidence,
            } => Text::new(
                format!(
                    "pane {pane} は pane {predecessor} からの引き継ぎを受け取っている\
                     （送達の顛末: {evidence}）ので閉じない（force でも同じ。#2330）。\
                     後任は手順どおり前任を閉じるので、閉じ直さずに待つ。\
                     後任が動いていないように見えるなら、tako_read_pane で pane {pane} の\
                     画面・delivery・mod_turn を確かめる（guide handoff）"
                ),
                format!(
                    "pane {pane} has already received the handoff from pane {predecessor} \
                     (delivery outcome: {evidence}), so it is not closed (force does not override \
                     this. #2330). The successor closes the predecessor itself; wait instead of \
                     restarting the handoff. If the successor looks stuck, read pane {pane} with \
                     tako_read_pane first (screen, delivery, mod_turn; guide handoff)"
                ),
            ),
        }
    }
}

/// close を通すか（`Err` = 断る）。判断の 1 実装（dispatch の close と単体テストが引く）
pub fn decide(q: &CloseQuery<'_>) -> Result<(), Refusal> {
    let Some(profile) = q.target_role.and_then(master_profile_of_any_role) else {
        return Ok(()); // 対象が master でない
    };
    let caller_is_master = [q.caller_role, q.caller_pane_role]
        .into_iter()
        .flatten()
        .any(|r| master_profile_of_any_role(r).is_some());
    if !caller_is_master {
        return Ok(()); // 人の CLI・worker・role なし
    }
    if q.caller_pane == Some(q.target) {
        return Ok(()); // 自己片付け
    }
    // 後任が自分の前任を閉じる（引き継ぎの完了）。呼び出し元のペインが生きた master のとき
    // だけ（閉じられた後任の名乗りで前任を閉じる 10/18 の 2 本目はここを通さない）
    let caller_alive_master = q
        .caller_pane_role
        .and_then(master_profile_of_any_role)
        .is_some();
    if caller_alive_master && q.caller_predecessor == Some(q.target) {
        return Ok(());
    }
    if let Some((predecessor, evidence)) = q.received_from {
        return Err(Refusal::ReceivedSuccessor {
            pane: q.target,
            predecessor,
            evidence: evidence.to_string(),
        });
    }
    let others = q
        .tab_roles
        .iter()
        .filter(|r| r.and_then(master_profile_of_any_role) == Some(profile))
        .count();
    if others == 0 {
        return Err(Refusal::LastMaster {
            pane: q.target,
            profile: profile.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    const M1: &str = "orchestrator-master:p1";
    const M2: &str = "orchestrator-master:p2";

    fn query<'a>(target_role: &'a str, tab_roles: Vec<Option<&'a str>>) -> CloseQuery<'a> {
        CloseQuery {
            target: 10,
            target_role: Some(target_role),
            tab_roles,
            caller_pane: Some(20),
            caller_role: Some("master:p1"),
            caller_pane_role: Some(M1),
            received_from: None,
            caller_predecessor: None,
        }
    }

    /// 10/18 の 2 本目: 先に閉じられた側の名乗りで、タブに残った最後の master を閉じる
    #[test]
    fn タブの最後のmasterを閉じるcloseは断る() {
        let q = query(M1, vec![None, Some("orchestrator-worker:tako:#1")]);
        let err = decide(&q).unwrap_err();
        assert_eq!(err.code(), "last_master");
        assert_eq!(
            err,
            Refusal::LastMaster {
                pane: 10,
                profile: "p1".into()
            }
        );
        let ja = err.note().text_in(Lang::Ja).to_string();
        assert!(ja.contains("pane 10") && ja.contains("p1"), "{ja}");
        assert!(ja.contains("画面から閉じる"), "次の一手が無い: {ja}");
        assert!(err.note().text_in(Lang::En).contains("from the UI"));
    }

    /// 境界: 同じタブ・同じプロファイルに 2 人目が居る（引き継ぎの途中で前任が後任を閉じ直す等）
    #[test]
    fn 同じプロファイルの2人目が残るcloseは通す() {
        let q = query(M1, vec![Some(M1), None]);
        assert_eq!(decide(&q), Ok(()));
        // 表示用の既定と env 用の既定は同じプロファイル（orchestrator-master と master）
        let q = CloseQuery {
            caller_role: Some("master"),
            caller_pane_role: Some("orchestrator-master"),
            ..query("orchestrator-master", vec![Some("orchestrator-master")])
        };
        assert_eq!(decide(&q), Ok(()));
    }

    /// 別のプロファイルの master が残っても、そのプロファイルの master は居なくなる
    #[test]
    fn 別プロファイルのmasterしか残らないcloseは断る() {
        let q = query(M1, vec![Some(M2)]);
        assert_eq!(decide(&q).unwrap_err().code(), "last_master");
    }

    #[test]
    fn 自己片付けは最後の1人でも通す() {
        let q = CloseQuery {
            caller_pane: Some(10),
            ..query(M1, vec![])
        };
        assert_eq!(decide(&q), Ok(()));
    }

    /// master 以外の呼び出し（人の CLI・worker・role なし）は従来どおり
    #[test]
    fn masterでない呼び出し元は守りの対象外() {
        for (role, pane_role) in [
            (None, None),
            (Some("worker:tako"), Some("orchestrator-worker:tako")),
            (Some("solo"), None),
        ] {
            let q = CloseQuery {
                caller_role: role,
                caller_pane_role: pane_role,
                ..query(M1, vec![])
            };
            assert_eq!(decide(&q), Ok(()), "{role:?}");
        }
    }

    /// 名乗り（env）を失った master も、ペインの role で master と分かる（#854 と同じ事情）
    #[test]
    fn 名乗りを失ったmasterもペインのroleで守りの対象() {
        let q = CloseQuery {
            caller_role: None,
            caller_pane_role: Some(M1),
            ..query(M1, vec![])
        };
        assert_eq!(decide(&q).unwrap_err().code(), "last_master");
        // 呼び出し元のペインが分からない（閉じられた等）でも名乗りがあれば対象
        let q = CloseQuery {
            caller_pane: None,
            caller_pane_role: None,
            ..query(M1, vec![])
        };
        assert_eq!(decide(&q).unwrap_err().code(), "last_master");
    }

    #[test]
    fn masterでない対象は守りの対象外() {
        for role in ["orchestrator-worker:tako:#1", "solo", ""] {
            let q = query(role, vec![]);
            assert_eq!(decide(&q), Ok(()), "{role}");
        }
        let q = CloseQuery {
            target_role: None,
            ..query(M1, vec![])
        };
        assert_eq!(decide(&q), Ok(()));
    }

    /// 10/18 の 1 本目: 後任は受け取っていたのに、前任が閉じ直した
    #[test]
    fn 受領済みの後任は前任からも閉じない() {
        let q = CloseQuery {
            received_from: Some((20, "received")),
            ..query(M1, vec![Some(M1)])
        };
        let err = decide(&q).unwrap_err();
        assert_eq!(err.code(), "received_successor");
        let ja = err.note().text_in(Lang::Ja).to_string();
        assert!(ja.contains("pane 10") && ja.contains("pane 20"), "{ja}");
        assert!(ja.contains("received"), "受領の証拠が出ない: {ja}");
        assert!(ja.contains("tako_read_pane"), "確かめ方が無い: {ja}");
        // 後任が自分で閉じるのは止めない
        let q = CloseQuery {
            caller_pane: Some(10),
            received_from: Some((20, "received")),
            ..query(M1, vec![Some(M1)])
        };
        assert_eq!(decide(&q), Ok(()));
    }

    /// 後任が別のタブに立った引き継ぎ（`--tab`・後から動かした後任）: 前任のタブには同じ
    /// プロファイルの master が残らないが、後任が前任を閉じる close は通す（断ると前任が幽霊になる）
    #[test]
    fn 別タブの後任が自分の前任を閉じるcloseは通す() {
        let q = CloseQuery {
            caller_predecessor: Some(10),
            ..query(M1, vec![None])
        };
        assert_eq!(decide(&q), Ok(()));
        // 帳面に無い別タブの master（誰の後任でもない / 別の前任の後任）は従来どおり最後の master
        for other in [None, Some(99)] {
            let q = CloseQuery {
                caller_predecessor: other,
                ..query(M1, vec![None])
            };
            assert_eq!(decide(&q).unwrap_err().code(), "last_master", "{other:?}");
        }
        // 閉じられた後任（ペインが無い = role が引けない）の名乗りでは通さない（10/18 の 2 本目）
        let q = CloseQuery {
            caller_pane_role: None,
            caller_predecessor: Some(10),
            ..query(M1, vec![None])
        };
        assert_eq!(decide(&q).unwrap_err().code(), "last_master");
    }

    /// 帳面: 発行 → 受領 → 前任を閉じる（完了）/ 後任を閉じる
    #[test]
    fn 帳面は発行と受領と完了を追う() {
        let mut book = HandoffBook::default();
        assert!(!book.note_received(3, "verified"), "帳面に無いペインは無視");
        book.issue(3, 1);
        assert_eq!(book.get(3).unwrap().received, None);
        assert!(book.note_received(3, "received"));
        assert!(!book.note_received(3, "verified"), "最初の証拠を保つ");
        assert_eq!(book.get(3).unwrap().received.as_deref(), Some("received"));
        // 前任を閉じた = 引き継ぎの完了
        book.forget(1);
        assert!(book.get(3).is_none());
        // 後任を閉じた（受領前に前任がやり直す）
        book.issue(5, 4);
        book.forget(5);
        assert!(book.get(5).is_none());
    }
}

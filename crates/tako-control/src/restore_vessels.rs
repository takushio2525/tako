//! 復元で器（tmux セッション）の名前を表のペインのどれが持つか（Issue #2486）。
//!
//! # 何が壊れていたか
//!
//! `layout.json` で 2 つ以上の表のペインが**同じ器の名前**を持つと、復元ループ（`tako-app` の
//! 起動処理）は両方を同じ器へ繋いでいた。器のクライアントは `new-session -A -D`
//! （`tako_core::tmux_backend::wrap_options_for`）なので、2 本目が 1 本目のクライアントを
//! 切り離し、1 本目は #1857 の判定で「別のクライアントが attach 中」として**ペインごと閉じる**
//! （3 本なら 2 本が閉じる）。persist.log には #1857 の「別のクライアント 1 本が attach 中なので
//! 閉じる」しか残らず、多重起動の後発に切り離されたのと区別できなかった。
//!
//! # 直し方
//!
//! 復元の順（タブの順）で最初のペインが器を持つ。2 本目以降は**その器の名前を使わず**、
//! 新しい器（`backend::wrap_spawn_for_pane` が払い出す = #885 の入口。#428 の名前の検査も通る）で
//! 保存 cwd の新しいシェルを起こす。ペインは消えず、1 つの器を 2 本の端末で取り合わない
//! （どちらかを閉じたときにもう一方の器ごと kill される #428 型の取り違えも作らない）。
//! 何をしたかは persist.log へ 1 行（[`log_line`]）。
//!
//! 会話の resume は次のときにしない（[`DuplicatePlan::resume_blocked`]）:
//! - 器が生きている: 器の中で動いているのがどちらのペインの会話か分からない。持ち主のペインが
//!   器ごと戻すので、resume すると同じ会話を 2 本の agent で動かしうる
//! - 持ち主のペインと同じ会話を指す: 持ち主が resume する
//!
//! 会話そのものは消えない（会話の記録とカタログに残る = `tako sessions resume` で戻せる）。
//! 2 本目が前に映していた画面は器が 1 つしか無いので戻らない（器が生きていれば持ち主のペインに映る）。
//!
//! 退避（たまり場・退避タブ）の重複は `shelved_restore::plan` が外す（#1576。表のペインが勝つ）。
//!
//! A/B は `TAKO_2486_LEGACY=1`（修正前 = 重複を見ずに同じ器へ繋ぐ）。

use std::collections::HashMap;

use crate::layout::RestoredPane;

/// #2486 の A/B。`TAKO_2486_LEGACY=1` で修正前の挙動（重複を見ずに同じ器へ繋ぐ）
pub fn legacy() -> bool {
    std::env::var_os("TAKO_2486_LEGACY").is_some()
}

/// 表のペインの器の名前の持ち主（復元の順で最初にその名前を持つペイン）
#[derive(Debug, Clone, Default)]
pub struct TabVessels {
    /// 器の名前 → 持ち主のペイン
    owners: HashMap<String, u64>,
    /// 持ち主のペインの会話（重複したペインの会話と比べる）
    conversations: HashMap<u64, Conversation>,
}

/// 表のペイン 1 本が器の名前をどう使うか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabVessel<'a> {
    /// 器の名前を持つ（名前が無いペインもここ）。この名前で器へ繋ぐ
    Own(Option<&'a str>),
    /// 同じ名前を先のペインが持つ。器の名前は使わずに新しい器で起こす
    Duplicate(DuplicatePlan),
}

impl<'a> TabVessel<'a> {
    /// このペインが繋ぐ器の名前（重複したペインは `None` = 新しい器を払い出させる）
    pub fn vessel(&self) -> Option<&'a str> {
        match self {
            Self::Own(name) => *name,
            Self::Duplicate(_) => None,
        }
    }

    /// 重複したペインなら、その起こし方
    pub fn duplicate(&self) -> Option<&DuplicatePlan> {
        match self {
            Self::Own(_) => None,
            Self::Duplicate(plan) => Some(plan),
        }
    }
}

/// 重複したペインの起こし方
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicatePlan {
    /// 重複した器の名前
    pub session: String,
    /// 先にその名前を持つペイン（器はこちらが持つ）
    pub holder: u64,
    /// 会話の resume をしない理由（`None` = resume してよい / 会話の記録が無い）
    pub resume_blocked: Option<ResumeBlock>,
}

/// 重複したペインで会話を resume しない理由
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeBlock {
    /// 器が生きている（中で動いているのがどちらの会話か分からない）
    VesselAlive,
    /// 持ち主のペインと同じ会話を指す
    SameConversation,
}

impl ResumeBlock {
    /// 診断ログ用の日本語ラベル
    pub const fn label(self) -> &'static str {
        match self {
            Self::VesselAlive => "器が生きていて中の会話がどちらのものか分からない",
            Self::SameConversation => "先のペインと同じ会話",
        }
    }
}

/// ペインの会話（系統と ID）。claude は `claude_session_id`、他系統は `agent_resume`
#[derive(Debug, Clone, PartialEq, Eq)]
struct Conversation {
    agent: String,
    id: String,
}

fn conversation_of(pane: &RestoredPane) -> Option<Conversation> {
    if let Some(id) = &pane.claude_session_id {
        return Some(Conversation {
            agent: "claude".into(),
            id: id.clone(),
        });
    }
    let saved = pane.agent_resume.as_ref()?;
    Some(Conversation {
        agent: saved.agent.clone(),
        id: saved.id.clone()?,
    })
}

/// 会話の記録を持つか（ID の無い系統だけの記録も含む = 復元で resume を試みる材料）
fn has_conversation(pane: &RestoredPane) -> bool {
    pane.claude_session_id.is_some() || pane.agent_resume.is_some()
}

/// 器を使うペインか（プレビュー / Web ビューは PTY を起こさないので器を取り合わない）
fn uses_vessel(pane: &RestoredPane) -> bool {
    pane.preview.is_none() && pane.webview.is_none()
}

impl TabVessels {
    /// 表のペイン（`in_tabs`）の器の名前の持ち主を決める。**復元ループと同じ順**
    /// （`restored` の並び）で最初にその名前を持つペインが持ち主
    pub fn new(restored: &[RestoredPane], in_tabs: impl Fn(u64) -> bool) -> Self {
        let mut vessels = Self::default();
        for pane in restored
            .iter()
            .filter(|p| in_tabs(p.pane) && uses_vessel(p))
        {
            let Some(name) = &pane.session else {
                continue;
            };
            if !vessels.owners.contains_key(name) {
                vessels.owners.insert(name.clone(), pane.pane);
                if let Some(conversation) = conversation_of(pane) {
                    vessels.conversations.insert(pane.pane, conversation);
                }
            }
        }
        vessels
    }

    /// 表のペイン 1 本の器の名前の使い方を決める。
    ///
    /// `vessel_alive` はその名前の器が今生きているかを問う（会話の記録を持つ重複したペインのときだけ呼ぶ）。
    /// 退避のペイン・器を使わないペイン・名前の無いペインは常に `Own`
    pub fn claim<'a>(
        &self,
        pane: &'a RestoredPane,
        in_tabs: bool,
        vessel_alive: impl FnOnce(&str) -> bool,
    ) -> TabVessel<'a> {
        let name = pane.session.as_deref();
        let Some(session) = name.filter(|_| in_tabs && uses_vessel(pane)) else {
            return TabVessel::Own(name);
        };
        let holder = match self.owners.get(session) {
            Some(&holder) if holder != pane.pane => holder,
            _ => return TabVessel::Own(name),
        };
        let resume_blocked = if !has_conversation(pane) {
            None
        } else if vessel_alive(session) {
            Some(ResumeBlock::VesselAlive)
        } else if conversation_of(pane).is_some_and(|c| self.conversations.get(&holder) == Some(&c))
        {
            Some(ResumeBlock::SameConversation)
        } else {
            None
        };
        TabVessel::Duplicate(DuplicatePlan {
            session: session.to_string(),
            holder,
            resume_blocked,
        })
    }
}

/// persist.log へ書く 1 行（FR-5.15 と同じ規約: ペイン ID・セッション名・理由だけ。
/// **ペインの内容・会話の ID は載せない**）
pub fn log_line(pane: u64, plan: &DuplicatePlan) -> String {
    let resume = match plan.resume_blocked {
        Some(block) => format!(
            "。会話は resume しない（{}。tako sessions list から戻せる）",
            block.label()
        ),
        None => String::new(),
    };
    format!(
        "器の名前の重複: pane={pane} session={} はペイン {} が先に持つので、\
         このペインは新しい器の新しいシェルで起こす（ペインは閉じない{resume}）",
        plan.session, plan.holder
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{AgentResumeLayout, PreviewLayout};

    fn pane(id: u64, session: Option<&str>) -> RestoredPane {
        RestoredPane {
            pane: id,
            session: session.map(str::to_string),
            cwd: None,
            claude_session_id: None,
            agent_resume: None,
            logged_history: None,
            preview: None,
            webview: None,
            ssh: None,
        }
    }

    fn with_claude(mut p: RestoredPane, id: &str) -> RestoredPane {
        p.claude_session_id = Some(id.into());
        p
    }

    fn all_tabs(_: u64) -> bool {
        true
    }

    fn alive(_: &str) -> bool {
        true
    }

    fn gone(_: &str) -> bool {
        false
    }

    /// 名前が重複しなければ全員が自分の器を持つ（修正前と同じ）
    #[test]
    fn 重複が無ければ全員が自分の器を持つ() {
        let restored = vec![
            pane(1, Some("tako-a")),
            pane(2, Some("tako-b")),
            pane(3, None),
        ];
        let vessels = TabVessels::new(&restored, all_tabs);
        for r in &restored {
            assert_eq!(
                vessels.claim(r, true, |_| panic!("重複でなければ器の生死を問わない")),
                TabVessel::Own(r.session.as_deref())
            );
        }
    }

    /// 2 本が同じ名前: 先のペインが器を持ち、2 本目は器の名前を使わない（#2486 の本体）
    #[test]
    fn 二本目は器の名前を使わない() {
        let restored = vec![pane(1, Some("tako-a")), pane(2, Some("tako-a"))];
        let vessels = TabVessels::new(&restored, all_tabs);
        let first = vessels.claim(&restored[0], true, alive);
        assert_eq!(first.vessel(), Some("tako-a"));
        assert!(first.duplicate().is_none());
        let second = vessels.claim(&restored[1], true, alive);
        assert_eq!(
            second.vessel(),
            None,
            "2 本目が同じ器へ繋ぐと 1 本目が閉じる"
        );
        assert_eq!(
            second.duplicate(),
            Some(&DuplicatePlan {
                session: "tako-a".into(),
                holder: 1,
                resume_blocked: None,
            })
        );
    }

    /// 3 本が同じ名前: 2 本目・3 本目とも先頭のペインを持ち主として名指す
    #[test]
    fn 三本目も先頭のペインを持ち主とする() {
        let restored = vec![
            pane(5, Some("tako-a")),
            pane(6, Some("tako-a")),
            pane(7, Some("tako-a")),
        ];
        let vessels = TabVessels::new(&restored, all_tabs);
        assert_eq!(
            vessels.claim(&restored[0], true, alive).vessel(),
            Some("tako-a")
        );
        for r in &restored[1..] {
            let claim = vessels.claim(r, true, alive);
            assert_eq!(claim.vessel(), None);
            assert_eq!(claim.duplicate().map(|d| d.holder), Some(5));
        }
    }

    /// 持ち主は復元の順で決まる（ペイン ID の大小ではない）
    #[test]
    fn 持ち主は復元の順で決まる() {
        let restored = vec![pane(9, Some("tako-a")), pane(2, Some("tako-a"))];
        let vessels = TabVessels::new(&restored, all_tabs);
        assert_eq!(
            vessels.claim(&restored[0], true, alive).vessel(),
            Some("tako-a")
        );
        assert_eq!(
            vessels
                .claim(&restored[1], true, alive)
                .duplicate()
                .map(|d| d.holder),
            Some(9)
        );
    }

    /// 退避のペインは `shelved_restore::plan` の領分（ここでは触らない）。
    /// 退避のペインが先に並んでいても、表のペインの持ち主にはならない
    #[test]
    fn 退避のペインは扱わない() {
        let restored = vec![pane(1, Some("tako-a")), pane(2, Some("tako-a"))];
        let vessels = TabVessels::new(&restored, |id| id == 2);
        assert_eq!(
            vessels.claim(&restored[0], false, alive),
            TabVessel::Own(Some("tako-a"))
        );
        assert_eq!(
            vessels.claim(&restored[1], true, alive),
            TabVessel::Own(Some("tako-a"))
        );
    }

    /// プレビュー / Web ビューは器を起こさないので持ち主にならず、重複にも数えない
    #[test]
    fn 器を使わないペインは取り合わない() {
        let mut preview = pane(1, Some("tako-a"));
        preview.preview = Some(PreviewLayout {
            path: "/tmp/a.md".into(),
            mode: "markdown".into(),
        });
        let restored = vec![preview, pane(2, Some("tako-a"))];
        let vessels = TabVessels::new(&restored, all_tabs);
        assert_eq!(
            vessels.claim(&restored[1], true, alive),
            TabVessel::Own(Some("tako-a"))
        );
        assert_eq!(
            vessels.claim(&restored[0], true, alive),
            TabVessel::Own(Some("tako-a"))
        );
    }

    /// 器が生きていれば会話は resume しない（中で動いているのがどちらの会話か分からない）
    #[test]
    fn 器が生きていれば会話はresumeしない() {
        let restored = vec![
            with_claude(pane(1, Some("tako-a")), "c-1"),
            with_claude(pane(2, Some("tako-a")), "c-2"),
        ];
        let vessels = TabVessels::new(&restored, all_tabs);
        assert_eq!(
            vessels
                .claim(&restored[1], true, alive)
                .duplicate()
                .and_then(|d| d.resume_blocked),
            Some(ResumeBlock::VesselAlive)
        );
    }

    /// 器が消えていれば、持ち主と違う会話は resume する・同じ会話はしない
    #[test]
    fn 器が消えていれば別の会話だけresumeする() {
        let restored = vec![
            with_claude(pane(1, Some("tako-a")), "c-1"),
            with_claude(pane(2, Some("tako-a")), "c-2"),
            with_claude(pane(3, Some("tako-a")), "c-1"),
        ];
        let vessels = TabVessels::new(&restored, all_tabs);
        assert_eq!(
            vessels
                .claim(&restored[1], true, gone)
                .duplicate()
                .map(|d| d.resume_blocked),
            Some(None)
        );
        assert_eq!(
            vessels
                .claim(&restored[2], true, gone)
                .duplicate()
                .and_then(|d| d.resume_blocked),
            Some(ResumeBlock::SameConversation)
        );
    }

    /// claude 以外（codex / agy）の会話も同じく比べる。系統が違えば別の会話
    #[test]
    fn 他系統の会話も比べる() {
        let codex = |id: u64, conv: &str| {
            let mut p = pane(id, Some("tako-a"));
            p.agent_resume = Some(AgentResumeLayout {
                agent: "codex".into(),
                id: Some(conv.into()),
            });
            p
        };
        let restored = vec![
            codex(1, "x-1"),
            codex(2, "x-1"),
            with_claude(pane(3, Some("tako-a")), "x-1"),
        ];
        let vessels = TabVessels::new(&restored, all_tabs);
        assert_eq!(
            vessels
                .claim(&restored[1], true, gone)
                .duplicate()
                .and_then(|d| d.resume_blocked),
            Some(ResumeBlock::SameConversation)
        );
        assert_eq!(
            vessels
                .claim(&restored[2], true, gone)
                .duplicate()
                .and_then(|d| d.resume_blocked),
            None
        );
    }

    /// 会話の記録が無いペインは器の生死を問わない（resume するものが無い）
    #[test]
    fn 会話の無いペインは器の生死を問わない() {
        let restored = vec![pane(1, Some("tako-a")), pane(2, Some("tako-a"))];
        let vessels = TabVessels::new(&restored, all_tabs);
        let claim = vessels.claim(&restored[1], true, |_| {
            panic!("会話の無いペインで器の生死を問わない")
        });
        assert_eq!(claim.duplicate().and_then(|d| d.resume_blocked), None);
    }

    /// persist.log の 1 行: ペイン・器の名前・持ち主・閉じないことを言い、会話の ID は載せない
    #[test]
    fn 理由の一行() {
        let plan = DuplicatePlan {
            session: "tako-a".into(),
            holder: 1,
            resume_blocked: Some(ResumeBlock::VesselAlive),
        };
        let line = log_line(2, &plan);
        assert!(
            line.starts_with("器の名前の重複: pane=2 session=tako-a "),
            "{line}"
        );
        assert!(line.contains("ペイン 1 が先に持つ"), "{line}");
        assert!(line.contains("ペインは閉じない"), "{line}");
        assert!(line.contains(ResumeBlock::VesselAlive.label()), "{line}");
        let plain = log_line(
            2,
            &DuplicatePlan {
                resume_blocked: None,
                ..plan
            },
        );
        assert!(!plain.contains("resume"), "{plain}");
    }
}

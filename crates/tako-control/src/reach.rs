//! reach — ペインへ「どう届くか」の解決（設計 §3.3。Issue #519 段取り ③）
//!
//! 設計の正: `.agent/plans/2026-07-windows-persistence-backend.md`
//!
//! tako がペインへ届く経路は 2 つしかない。
//!
//! 1. **in-process**: tako-app が当該ペインの `TerminalSession` を持っている。主経路であり、
//!    画面採取も入力も送達検証もすべてここで完結する（tmux に一切依存しない）
//! 2. **detached**: tako-app が持っていない（GUI 不在・ペイン消失）とき、
//!    永続バックエンドの [`DetachedAccess`] 越しに届く
//!
//! `dispatch` はこれまで「in-process で解決 → 失敗したら `tako_core::tmux` を直接叩く」
//! という形を約 13 箇所で個別に書いていた。**この規律は型のどこにも表現されておらず**、
//! バックエンドが到達手段を持たない環境（Windows の `NullBackend`）で
//! 「フォールバックが失敗した」のか「そもそも到達手段が無い」のかを呼び出し側が区別できなかった。
//!
//! [`PaneReach`] はその 3 状態を網羅 match で扱わせる。バリアントを増やすと
//! 全呼び出し側がコンパイルエラーになるため、フォールバック未記述の経路が
//! テストより早い段階で露出する。

use tako_core::backend::{backend, DetachedAccess, SessionRef};
use tako_core::PaneId;

use crate::host::ControlHost;

/// ペインへの到達手段
pub enum PaneReach {
    /// tako-app が当該ペインを保持している（主経路。全機能が使える）
    InProcess(PaneId),
    /// tako-app 不在 / ペイン消失。永続バックエンドの到達手段で届く
    Detached(SessionRef, &'static dyn DetachedAccess),
    /// 届かない
    Unreachable(UnreachableReason),
}

/// 届かない理由。**「フォールバックが失敗した」と「到達手段そのものが無い」を区別する**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnreachableReason {
    /// ペインが tako-app に無く、バックエンドセッションのヒントも無い
    NoSession(u64),
    /// ヒントはあるが、このバックエンドにはアウトオブプロセス到達手段が無い
    /// （Windows 初期リリース = `NullBackend`。設計 §4）
    NoDetachedAccess { session: String, note: String },
    /// セッション名として不正（#428 のターゲット式 `session:0.0` など）
    InvalidSession { hint: String, note: String },
}

impl UnreachableReason {
    /// 診断メッセージ。dispatch のエラー文字列はここから作る（二重管理を作らない）
    pub fn note(&self) -> String {
        match self {
            Self::NoSession(pane) => {
                format!("ペイン {pane} のセッションが無く、バックエンドセッションの指定も無い")
            }
            Self::NoDetachedAccess { session, note } => {
                format!("セッション {session} へ届かない: {note}")
            }
            Self::InvalidSession { hint, note } => {
                format!("セッション指定 '{hint}' が不正: {note}")
            }
        }
    }
}

/// このバックエンドに到達手段が無いことの説明。UI・エラー・診断で同じ文字列を使う
pub fn no_detached_access_note() -> String {
    let caps = backend().capabilities();
    format!(
        "永続バックエンド（{}）にアウトオブプロセス到達手段が無いため、\
         tako-app が保持していないペインの画面採取・入力送出はできない",
        caps.label
    )
}

impl PaneReach {
    /// 到達手段を解決する。
    ///
    /// `session_hint` は呼び出し側が持っているバックエンドセッション名
    /// （`Request` の `tmux_session` フィールド由来）。
    /// **in-process を必ず先に試す**のは、そちらが主経路であり全機能が使えるため。
    pub fn resolve(host: &dyn ControlHost, pane: Option<u64>, session_hint: Option<&str>) -> Self {
        if let Some(target) = in_process_pane(host, pane) {
            return Self::InProcess(target);
        }
        let Some(hint) = session_hint.filter(|s| !s.is_empty()) else {
            return Self::Unreachable(UnreachableReason::NoSession(pane.unwrap_or(0)));
        };
        let session = match SessionRef::new(hint) {
            Ok(s) => s,
            Err(e) => {
                return Self::Unreachable(UnreachableReason::InvalidSession {
                    hint: hint.to_string(),
                    note: e.to_string(),
                })
            }
        };
        match backend().detached() {
            Some(access) => Self::Detached(session, access),
            None => Self::Unreachable(UnreachableReason::NoDetachedAccess {
                session: session.into_string(),
                note: no_detached_access_note(),
            }),
        }
    }

    /// 到達手段のうち detached 側だけを取り出す（in-process 経路を別に書いた後の
    /// フォールバック節で使う）。`Ok(None)` は「in-process で処理済み」を意味する
    pub fn detached(&self) -> Option<(&SessionRef, &'static dyn DetachedAccess)> {
        match self {
            Self::Detached(s, a) => Some((s, *a)),
            _ => None,
        }
    }
}

/// tako-app が当該ペインの `TerminalSession` を持っているか
fn in_process_pane(host: &dyn ControlHost, pane: Option<u64>) -> Option<PaneId> {
    let (_, target) = crate::dispatch::resolve_pane(host.workspace(), pane).ok()?;
    host.session(target).map(|_| target)
}

// --- セッションヒント直指定の到達（pane 解決を伴わない経路） -----------------

/// バックエンドセッションが生きているか。
///
/// **器の有無を問う質問**なので、到達手段（`DetachedAccess`）が無い backend でも
/// 答えられる（`NullBackend` は常に false = 器が無いのだから当然）。
/// 不正なセッション名（#428 のターゲット式）は false へ倒す
pub fn session_alive(hint: &str) -> bool {
    match SessionRef::new(hint) {
        Ok(s) => backend().exists(&s),
        Err(_) => false,
    }
}

/// セッションヒントから到達手段を引く。
///
/// `None` は「そのセッションへは届かない」= 名前が不正か、backend に
/// アウトオブプロセス到達手段が無いか、のいずれか。
/// 理由まで要るなら [`PaneReach::resolve`] を使う
pub fn detached_session(hint: &str) -> Option<(SessionRef, &'static dyn DetachedAccess)> {
    let session = SessionRef::new(hint).ok()?;
    let access = backend().detached()?;
    Some((session, access))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 到達不能の理由は種別ごとに区別できる() {
        let no_session = UnreachableReason::NoSession(7);
        assert!(no_session.note().contains("ペイン 7"));

        let no_access = UnreachableReason::NoDetachedAccess {
            session: "tako-abc".into(),
            note: "テスト理由".into(),
        };
        assert!(no_access.note().contains("tako-abc"));
        assert!(no_access.note().contains("テスト理由"));

        // 「届かない」を 1 種類にまとめない = 呼び出し側が縮退と失敗を取り違えない
        assert_ne!(no_session, no_access);
    }

    /// #428 の回帰: ターゲット式をセッション名として渡しても
    /// tmux へ投げる前に構造で弾く
    #[test]
    fn ターゲット式のヒントは到達解決の時点で弾かれる() {
        let reason = match SessionRef::new("tako-abc:0.0") {
            Err(e) => UnreachableReason::InvalidSession {
                hint: "tako-abc:0.0".into(),
                note: e.to_string(),
            },
            Ok(_) => panic!("ターゲット式が通ってしまった"),
        };
        assert!(reason.note().contains("ターゲット式"));
    }

    /// 到達手段が無い理由の文面には、どの backend なのかが必ず入る
    #[test]
    fn 到達手段なしの説明にはbackend名が入る() {
        let note = no_detached_access_note();
        let label = backend().capabilities().label;
        assert!(note.contains(label), "note={note} label={label}");
    }
}

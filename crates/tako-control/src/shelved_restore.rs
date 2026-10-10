//! 退避（たまり場・退避タブ）のペインを復元でどう戻すか（Issue #1576 / FR-2.15.5 / FR-2.15.7）。
//!
//! # 何が壊れていたか
//!
//! 復元ループ（`tako-app` の起動処理）は「タブ配下に居ないペイン」を `let-else` で
//! 抜けていた（#1554 で「たまり場・退避」として数えるようにしたが、抜けること自体は残った）。
//! 抜けた先では器（tmux セッション）を `backend_sessions` へ登録しないので、
//!
//! 1. 起動時の orphan 自動復帰（#191）の protected 集合から漏れ、器が「復帰」タブへ
//!    **別 pane id** で拾われる（role・タイトル・limit_resume・worker レジストリの紐付けが切れ、
//!    人がそのタブを閉じると器ごと kill = 退避しておいた会話が消える。2026-09-23 / 10-09 の実害）
//! 2. 退避エントリは端末を持たない**幽霊**として残り、次の保存で器の名前が `null` になる
//!    （以後は二度と器と結び付かない）
//!
//! 「表に出すときに起こす」設計のつもりだったが、表に出す経路（`reattach_backgrounded`）は
//! 端末を起こさないので、幽霊は表に出しても幽霊のままだった。
//!
//! # 直し方
//!
//! 退避のペインも**表のペインと同じ経路で起こす**（器が生きていれば再 attach・消えていれば
//! 保存 cwd の新シェル + 会話の resume）。退避中も端末を持つのは再起動前と同じ状態
//! （退避は画面から外すだけで attach を切らない = FR-2.15.1）なので、退避一覧の画面・
//! `tako read` 系・自動復帰の検知もそのまま効く。起こせないものは**退避から外す**
//! （幽霊を作らない）。その判断がこの 1 実装（[`plan`]）で、呼び出し側は結果に従うだけ。
//!
//! A/B は `TAKO_1576_LEGACY=1`（修正前 = 退避のペインを起こさず器も登録しない）。

use std::collections::HashSet;

use crate::layout::RestoredPane;
use crate::restore_report::FailureReason;

/// #1576 の A/B。`TAKO_1576_LEGACY=1` で修正前の挙動（退避のペインを起こさない）
pub fn legacy() -> bool {
    std::env::var_os("TAKO_1576_LEGACY").is_some()
}

/// 退避のペイン 1 本をどう扱うか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiddenPlan {
    /// 表のペインと同じ経路で起こす（器へ再 attach / 新シェル + resume / プレビュー）
    Wake,
    /// 起こさずに残す（A/B の旧挙動と、Web ビューのペイン = wry の窓は Web ビュー dock の
    /// 仕組みが持つので端末の器とは別の話）
    Leave,
    /// 退避から外す。理由は失敗の分類として内訳と persist.log に残す
    Drop(FailureReason),
}

/// 退避のペイン 1 本の扱いを決める。
///
/// - `claimed`: ここまでに起こす / 起こしたペインが持つ器の名前。**タブ配下のペインの器を
///   先に入れておく**（表のペインが勝つ。同じ器へ 2 本の端末を繋ぐと、どちらかを閉じた
///   ときにもう一方の器ごと kill される）。`Wake` を返したときはこのペインの器を足す
/// - `backend_persistent`: 器がアプリ終了を生き延びる環境か（tmux。`backend::capabilities`）。
///   そうでない環境（器なしの Windows 等）は器の名前が元から無いので、名前が無いことを
///   幽霊の印にしない（表のペインと同じく新しいシェルで戻す）
pub fn plan(
    pane: &RestoredPane,
    claimed: &mut HashSet<String>,
    backend_persistent: bool,
    legacy: bool,
) -> HiddenPlan {
    if legacy || pane.webview.is_some() {
        return HiddenPlan::Leave;
    }
    if let Some(name) = &pane.session {
        return if claimed.insert(name.clone()) {
            HiddenPlan::Wake
        } else {
            HiddenPlan::Drop(FailureReason::DuplicateVessel)
        };
    }
    // 器の名前が無い。器が永続する環境では、生きていた退避ペインは必ず名前を持つ
    // （`spawn_session` が登録する）ので、名前も戻す手掛かりも無いのは旧版の復元が
    // 残した幽霊（cwd も会話の記録も端末から取るので、端末の無い幽霊は全部 null）
    let has_clue = pane.cwd.is_some()
        || pane.claude_session_id.is_some()
        || pane.agent_resume.is_some()
        || pane.preview.is_some()
        || pane.ssh.is_some();
    if backend_persistent && !has_clue {
        HiddenPlan::Drop(FailureReason::NoVessel)
    } else {
        HiddenPlan::Wake
    }
}

/// タブ配下のペインが持つ器の名前（[`plan`] の `claimed` の初期値）
pub fn claimed_by_tabs<'a>(
    restored: &'a [RestoredPane],
    in_tabs: impl Fn(u64) -> bool + 'a,
) -> HashSet<String> {
    restored
        .iter()
        .filter(|r| in_tabs(r.pane))
        .filter_map(|r| r.session.clone())
        .collect()
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

    /// 器を持つ退避は起こす（#1576 の本体）。起こしたペインの器は `claimed` へ入る
    #[test]
    fn 器を持つ退避は起こす() {
        let mut claimed = HashSet::new();
        assert_eq!(
            plan(&pane(10, Some("tako-a")), &mut claimed, true, false),
            HiddenPlan::Wake
        );
        assert!(claimed.contains("tako-a"));
        // 器が消えていても起こす（器が無いことは spawn 側が `restore_plan` で判断する
        // = 表のペインと同じく新シェル + resume）
        assert_eq!(
            plan(&pane(11, Some("tako-gone")), &mut claimed, true, false),
            HiddenPlan::Wake
        );
    }

    /// 同じ器を 2 つのエントリが指す古い layout: 先に起こした側（タブ配下が先）が勝つ
    #[test]
    fn 同じ器を指す二本目は外す() {
        let restored = vec![pane(1, Some("tako-shared")), pane(2, Some("tako-b"))];
        let mut claimed = claimed_by_tabs(&restored, |id| id == 1);
        assert_eq!(claimed.len(), 1);
        assert_eq!(
            plan(&pane(20, Some("tako-shared")), &mut claimed, true, false),
            HiddenPlan::Drop(FailureReason::DuplicateVessel),
            "表のペインが持つ器を退避が取り合っている"
        );
        // 退避どうしの重複も 2 本目を外す
        assert_eq!(
            plan(&pane(21, Some("tako-x")), &mut claimed, true, false),
            HiddenPlan::Wake
        );
        assert_eq!(
            plan(&pane(22, Some("tako-x")), &mut claimed, true, false),
            HiddenPlan::Drop(FailureReason::DuplicateVessel)
        );
    }

    /// 器も手掛かりも無いエントリ（旧版の復元が残した幽霊）は外す
    #[test]
    fn 幽霊は外す() {
        let mut claimed = HashSet::new();
        assert_eq!(
            plan(&pane(30, None), &mut claimed, true, false),
            HiddenPlan::Drop(FailureReason::NoVessel)
        );
        assert!(claimed.is_empty());
    }

    /// 器の名前が無くても戻す手掛かりがあれば起こす（会話は resume で戻る）
    #[test]
    fn 手掛かりがあれば起こす() {
        let mut claimed = HashSet::new();
        let mut with_cwd = pane(40, None);
        with_cwd.cwd = Some("/tmp".into());
        assert_eq!(plan(&with_cwd, &mut claimed, true, false), HiddenPlan::Wake);
        let mut with_claude = pane(41, None);
        with_claude.claude_session_id = Some("0f3c".into());
        assert_eq!(
            plan(&with_claude, &mut claimed, true, false),
            HiddenPlan::Wake
        );
        let mut with_agent = pane(42, None);
        with_agent.agent_resume = Some(AgentResumeLayout {
            agent: "codex".into(),
            id: None,
        });
        assert_eq!(
            plan(&with_agent, &mut claimed, true, false),
            HiddenPlan::Wake
        );
        let mut preview = pane(43, None);
        preview.preview = Some(PreviewLayout {
            path: "/tmp/a.md".into(),
            mode: "markdown".into(),
        });
        assert_eq!(plan(&preview, &mut claimed, true, false), HiddenPlan::Wake);
    }

    /// 器が永続しない環境（器なし）では、名前が無いのは普通なので幽霊扱いしない
    #[test]
    fn 器が永続しない環境では名前なしを外さない() {
        let mut claimed = HashSet::new();
        assert_eq!(
            plan(&pane(50, None), &mut claimed, false, false),
            HiddenPlan::Wake
        );
    }

    /// Web ビューと A/B の旧挙動は起こさない
    #[test]
    fn webビューと旧挙動は起こさない() {
        let mut claimed = HashSet::new();
        let mut web = pane(60, Some("tako-w"));
        web.webview = Some("https://example.com".into());
        assert_eq!(plan(&web, &mut claimed, true, false), HiddenPlan::Leave);
        assert_eq!(
            plan(&pane(61, Some("tako-l")), &mut claimed, true, true),
            HiddenPlan::Leave
        );
        assert!(
            claimed.is_empty(),
            "起こさないペインの器を claimed に入れた"
        );
    }
}

//! 利用上限による停止の検知（Issue #813）。
//!
//! 判断そのもの（いつ動くか・何を選ぶか）は `tako_core::limit_resume` の純関数が持つ。
//! ここは**既存の検知をそのまま束ねて**「上限で止まっているか」を 1 つの型で返す層で、
//! 新しい検知規則は増やさない（増やすと #748 / #157 と食い違う）。
//!
//! - ダイアログ型: [`crate::claude_tui::detect_choice_dialog`] の `DialogKind::UsageLimit`
//!   （#748。「What do you want to do?」+「… wait for limit to reset」）
//! - idle 型: [`crate::orchestrator::wait::detect_worker_error`] の `UsageLimit`
//!   （#157。「usage limit reached」「hit your usage limit」「limit reached … reset」）
//!
//! **permission ダイアログ・API エラー・通常の idle では None を返す**のが本モジュールの
//! 責務（自動復帰は上限由来の停止に限る、という安全条件を 1 か所で守る）

use tako_core::limit_resume::{parse_reset_at, LimitStop, LimitStopKind};

use crate::claude_tui::DialogKind;
use crate::orchestrator::wait::{detect_worker_error, WorkerErrorKind};

/// 画面から「利用上限で止まっているか」を判定する。
///
/// `observed_at` はこの画面を観測した時刻（unix 秒）、`tz_offset` はローカルタイムの
/// UTC オフセット（秒）。リセット時刻は日付を持たない表記なので、**観測時刻から見て
/// 次に来る同じ時刻**として解決する（`tako_core::limit_resume::parse_reset_at`）
pub fn detect_limit_stop(lines: &[String], observed_at: i64, tz_offset: i32) -> Option<LimitStop> {
    // ダイアログが入力欄を奪っているならそちらが優先（idle 判定より確実）。
    // usage_limit 以外のダイアログ（permission / plan 確認 / model 選択）では
    // **何も返さない** = 自動復帰は発動しない
    if let Some(dialog) = crate::claude_tui::detect_choice_dialog(lines) {
        if dialog.kind != DialogKind::UsageLimit {
            return None;
        }
        // 根拠は本文（「Your limit will reset at 3am」がここに入る）+ 画面全体。
        // 本文がスクロールアウトしていても選択肢だけで上限とは分かるので、
        // リセット時刻が読めなくても停止としては返す（時刻不明の扱いは core 側）
        let message = if dialog.title.is_empty() {
            dialog
                .options
                .first()
                .map(|o| o.label.clone())
                .unwrap_or_default()
        } else {
            dialog.title.clone()
        };
        return Some(LimitStop {
            kind: LimitStopKind::Dialog,
            message,
            reset_at: reset_at_from_lines(lines, observed_at, tz_offset),
        });
    }

    // ダイアログ無し。画面末尾の異常停止パターンから usage_limit だけを拾う
    // （api_error / limit_dialog（codex のモデル切替提案）では発動しない）
    let joined = lines.join("\n");
    match detect_worker_error(&joined) {
        Some((WorkerErrorKind::UsageLimit, message)) => Some(LimitStop {
            reset_at: parse_reset_at(&message, observed_at, tz_offset)
                .or_else(|| reset_at_from_lines(lines, observed_at, tz_offset)),
            kind: LimitStopKind::Idle,
            message,
        }),
        _ => None,
    }
}

/// 画面のどこかにあるリセット時刻表記を拾う（下の行ほど新しいので末尾から見る）
fn reset_at_from_lines(lines: &[String], observed_at: i64, tz_offset: i32) -> Option<i64> {
    lines
        .iter()
        .rev()
        .find_map(|l| parse_reset_at(l, observed_at, tz_offset))
}

/// 現在の画面から自動復帰の材料をまとめて採る（GUI の 2 秒 tick 用）。
///
/// 画面の同一性判定に使う指紋も一緒に返す。**上限メッセージが出ている領域だけ**では
/// なく画面全体を材料にするので、生成中（= 画面が動いている）かどうかを
/// 文言に頼らず判定できる（#572 の教訓）
pub fn screen_fingerprint(lines: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in lines {
        l.trim_end().hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(s: &str) -> Vec<String> {
        s.lines().map(|l| l.to_string()).collect()
    }

    /// #748 の実採取 fixture と同じ limit 対処ダイアログ
    const LIMIT_DIALOG: &str = r#"⏺ 続けて実装します
  ⎿  Claude usage limit reached. Your limit will reset at 3am.

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   What do you want to do?

   ❯ 1. Stop and wait for limit to reset
     2. Upgrade to Max 20x for higher session limits every month
     3. Continue with usage credits

   Enter to confirm · Esc to cancel"#;

    /// ダイアログ無しで上限メッセージだけが出て止まっている画面（#157 の検知対象）
    const LIMIT_IDLE: &str = r#"⏺ 実装を進めます
  ⎿  Claude usage limit reached. Your limit will reset at 3am.

╭──────────────────────────────────────────────────────────────────────╮
│ >                                                                    │
╰──────────────────────────────────────────────────────────────────────╯
  [Opus 5 · 32%]"#;

    /// permission ダイアログ（自動復帰の対象外）
    const PERMISSION: &str = r#"╭──────────────────────────────────────────────────────────────────────╮
│ Bash command                                                         │
│ npm test                                                             │
│ Do you want to proceed?                                              │
│ ❯ 1. Yes                                                             │
│   2. Yes, and don't ask again                                        │
│   3. No, and tell Claude what to do differently                      │
╰──────────────────────────────────────────────────────────────────────╯"#;

    /// API エラーで止まっている画面（自動復帰の対象外 = supervisor の担当）
    const API_ERROR: &str = r#"⏺ 実装を進めます
  ⎿  API Error: Connection closed mid-response.

╭──────────────────────────────────────────────────────────────────────╮
│ >                                                                    │
╰──────────────────────────────────────────────────────────────────────╯"#;

    /// 通常の idle（作業完了。上限ではない）
    const NORMAL_IDLE: &str = r#"⏺ 実装が完了しました。テストは全て緑です。

╭──────────────────────────────────────────────────────────────────────╮
│ > Try "fix the failing test"                                         │
╰──────────────────────────────────────────────────────────────────────╯
  [Opus 5 · 12%]"#;

    /// JST 00:30 に相当する unix 秒（テストは実機のタイムゾーンに依存しない）
    const JST: i32 = 9 * 3600;
    const OBSERVED: i64 = 1_786_752_000 - 9 * 3600 + 30 * 60;

    #[test]
    fn issue813_ダイアログ型の上限停止をリセット時刻つきで検知する() {
        let stop = detect_limit_stop(&screen(LIMIT_DIALOG), OBSERVED, JST).expect("検知される");
        assert_eq!(stop.kind, LimitStopKind::Dialog);
        assert_eq!(
            stop.reset_at,
            Some(1_786_752_000 - 9 * 3600 + 3 * 3600),
            "本文の「reset at 3am」を拾う"
        );
    }

    #[test]
    fn issue813_idle型の上限停止をリセット時刻つきで検知する() {
        let stop = detect_limit_stop(&screen(LIMIT_IDLE), OBSERVED, JST).expect("検知される");
        assert_eq!(stop.kind, LimitStopKind::Idle);
        assert!(stop.message.contains("usage limit reached"));
        assert_eq!(stop.reset_at, Some(1_786_752_000 - 9 * 3600 + 3 * 3600));
    }

    #[test]
    fn issue813_上限以外の画面では検知しない() {
        for (name, src) in [
            ("permission ダイアログ", PERMISSION),
            ("API エラー", API_ERROR),
            ("通常の idle", NORMAL_IDLE),
        ] {
            assert!(
                detect_limit_stop(&screen(src), OBSERVED, JST).is_none(),
                "{name} で自動復帰が発動しうる状態になっている"
            );
        }
    }

    #[test]
    fn issue813_リセット時刻が読めなくても停止としては検知する() {
        let src = LIMIT_DIALOG.replace("Your limit will reset at 3am.", "Usage limit reached.");
        let stop = detect_limit_stop(&screen(&src), OBSERVED, JST).expect("検知される");
        assert_eq!(stop.kind, LimitStopKind::Dialog);
        assert_eq!(stop.reset_at, None, "不明は None（core 側で猶予に落ちる）");
    }

    #[test]
    fn issue813_画面指紋は内容が変われば変わり末尾空白では変わらない() {
        let a = screen(LIMIT_IDLE);
        let mut b = a.clone();
        assert_eq!(screen_fingerprint(&a), screen_fingerprint(&b));
        // 末尾空白の揺れ（カーソル移動等）では変わらない
        b[0] = format!("{}   ", b[0]);
        assert_eq!(screen_fingerprint(&a), screen_fingerprint(&b));
        // 中身が変われば変わる（= 生成中は静止と判定されない）
        b[0] = "⏺ 別の出力".to_string();
        assert_ne!(screen_fingerprint(&a), screen_fingerprint(&b));
    }

    #[test]
    fn issue813_上限ダイアログから安全な選択肢をラベルで選ぶ() {
        // 検知 → 選択肢の選別まで、実採取 fixture で通しで確かめる
        let dialog = crate::claude_tui::detect_choice_dialog(&screen(LIMIT_DIALOG)).expect("検知");
        let options: Vec<(Option<u32>, String)> = dialog
            .options
            .iter()
            .map(|o| (o.number, o.label.clone()))
            .collect();
        assert_eq!(
            tako_core::limit_resume::safe_choice(&options),
            Some((1, "Stop and wait for limit to reset"))
        );
    }
}

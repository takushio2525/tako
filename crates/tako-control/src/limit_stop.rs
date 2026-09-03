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
    detect_limit_stop_with(lines, observed_at, tz_offset, None)
}

/// 構造化ソースの手がかりを添えて判定する（#985）。
///
/// `hint` は codex の rollout（`codex_session::rate_limits_for_backend`）から採った
/// **正確なリセット時刻**。画面の文言パースは版ごとの書式に依存する
/// （codex 0.150.1 は同日なら `4:24 AM`、日をまたぐと `Aug 28th, 2026 4:24 AM`）のに対し、
/// こちらは epoch 秒なので**書式にもタイムゾーンにも依存しない**。
///
/// **手がかりだけでは停止と判定しない**のが要点。停止の根拠は画面のままにしておくことで
/// 「上限由来の停止に限る」という #813 の安全条件を 1 か所で守り続ける
/// （claude 経路は `hint = None` で通るので 1 ビットも変わらない）
pub fn detect_limit_stop_with(
    lines: &[String],
    observed_at: i64,
    tz_offset: i32,
    hint: Option<&LimitHint>,
) -> Option<LimitStop> {
    let mut stop = detect_limit_stop_from_screen(lines, observed_at, tz_offset)?;
    // 構造化ソースの時刻が読めていれば**そちらを採る**（文言パースより確か）
    if let Some(at) = hint.and_then(|h| h.reset_at) {
        stop.reset_at = Some(at);
    }
    Some(stop)
}

/// 構造化ソースから採った手がかり（#985）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimitHint {
    /// 上限が解ける時刻（unix 秒）。`None` = 構造化ソースでも分からない
    pub reset_at: Option<i64>,
}

impl LimitHint {
    /// codex の `rate_limits` から手がかりを作る。
    /// **上限に当たっている枠が無ければ空**（= 手がかり無し）
    pub fn from_codex(rl: &crate::codex_session::RateLimits) -> Self {
        Self {
            reset_at: rl.reset_at(),
        }
    }
}

fn detect_limit_stop_from_screen(
    lines: &[String],
    observed_at: i64,
    tz_offset: i32,
) -> Option<LimitStop> {
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

    // --- #1093: 組織クレジット上限（session limit） ---

    /// **実採取**（2026-09-03。univ アカウントの worker 3 体が 17:1x に止まっていた画面）。
    ///
    /// 上限の見出しは claude 2.1.258 のテンプレート
    /// `` `You've hit your ${限度の名前}${理由}` `` から作られ、限度の名前は
    /// `session limit`（= 5h 枠）、2 行目は組織で管理者へ依頼する場合の案内
    /// （どちらも同バイナリ内の文字列と一致）。
    ///
    /// **フッターに `5h NN%` / `7d NN%` が無い**のも実際の症状どおり
    /// （ステータスバーのメーターが `--` になっていた）。入力欄・区切り線の形は
    /// 稼働中の実ペインから採った幾何（フッター 6 行 / 入力欄 3 行）に合わせてある
    const SESSION_LIMIT_IDLE: &str = r#"⏺ 実装を進めます

  ⎿  You've hit your session limit · resets 7:50pm (Asia/Tokyo)
     /usage-credits to request more usage from your admin.

────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────
  ⏵⏵ accept edits on
  tako
  main
  ~/dev/tako
  ⏸ 待機中 · ? for shortcuts"#;

    #[test]
    fn issue1093_組織クレジット上限で止まった画面を解除時刻つきで検知する() {
        let stop =
            detect_limit_stop(&screen(SESSION_LIMIT_IDLE), OBSERVED, JST).expect("検知される");
        assert_eq!(
            stop.kind,
            LimitStopKind::Idle,
            "ダイアログは出ていないので idle 型"
        );
        assert!(
            stop.message.contains("hit your session limit"),
            "検知の根拠が見出し行になっていない: {}",
            stop.message
        );
        assert_eq!(
            stop.reset_at,
            Some(1_786_752_000 - 9 * 3600 + 19 * 3600 + 50 * 60),
            "`resets 7:50pm (Asia/Tokyo)` から解除時刻が読めていない"
        );
    }

    #[test]
    fn issue1093_解除時刻の無い組織上限でも停止としては検知する() {
        // 管理者へ依頼する案内だけで解除時刻が出ない形（バイナリ内 `Tet()` 系）。
        // 時刻不明は core 側の猶予（`UNKNOWN_RESET_FALLBACK_SECS`）に落ちる
        let src = SESSION_LIMIT_IDLE.replace(
            "· resets 7:50pm (Asia/Tokyo)",
            "· run /usage-credits to ask your admin for a higher limit",
        );
        let stop = detect_limit_stop(&screen(&src), OBSERVED, JST).expect("検知される");
        assert_eq!(stop.kind, LimitStopKind::Idle);
        assert_eq!(stop.reset_at, None);
    }

    #[test]
    fn issue1093_週枠の上限も同じ経路で検知する() {
        // `nF` 表の `seven_day` = `weekly limit`。同日中の解除なら日付が付かない
        let src = SESSION_LIMIT_IDLE.replace("session limit", "weekly limit");
        let stop = detect_limit_stop(&screen(&src), OBSERVED, JST).expect("検知される");
        assert_eq!(
            stop.reset_at,
            Some(1_786_752_000 - 9 * 3600 + 19 * 3600 + 50 * 60)
        );
    }

    // --- #985: codex ---

    /// codex 0.150.1 の上限停止画面（**日付つきの `Try again at`**）。
    /// 書式はバイナリ内の `" Try again at "` + `", %Y %-I:%M %p"` から。
    /// 到達文言（`You've hit your usage limit.`）も同じくバイナリ内文字列
    const CODEX_LIMIT_DATED: &str = r#"■ You've hit your usage limit. Upgrade to Pro
(https://chatgpt.com/explore/pro), visit
https://chatgpt.com/codex/settings/usage to purchase more credits or
try again at Aug 28th, 2026 4:24 AM.

›
"#;

    /// codex の「Approaching rate limits」ダイアログ（実採取。#157 / #748）
    const CODEX_APPROACHING: &str = r#"  Approaching rate limits
  Switch to gpt-5.4-mini for lower credit usage?

› 1. Switch to gpt-5.4-mini
  2. Keep current model
  3. Keep current model (never show again)

  Press enter to confirm or esc to go back"#;

    /// codex の `/usage` ダイアログ（**実採取**。2026-08-27 / 0.150.1）。
    /// 「Redeem usage limit reset」は在庫のあるリセットを引き換える = 自動確定禁止
    const CODEX_USAGE_MENU: &str = r#"  Usage
  View account usage or redeem an earned reset.

› 1. Show usage                View recent account token usage.
  2. Redeem usage limit reset  You have 1 usage limit reset available.

  Press enter to confirm or esc to go back"#;

    fn codex_limits(reset_at: Option<i64>) -> crate::codex_session::RateLimits {
        crate::codex_session::RateLimits {
            primary: Some(crate::codex_session::RateWindow {
                used_percent: 100,
                window_minutes: 300,
                resets_at: reset_at,
            }),
            secondary: None,
            plan_type: Some("plus".into()),
            reached: Some("primary".into()),
        }
    }

    #[test]
    fn issue985_codexの日付つき上限画面を解除時刻つきで検知する() {
        let stop =
            detect_limit_stop(&screen(CODEX_LIMIT_DATED), OBSERVED, JST).expect("検知される");
        assert_eq!(
            stop.kind,
            LimitStopKind::Idle,
            "codex の停止はダイアログ無し"
        );
        assert_eq!(
            stop.reset_at,
            Some(1_786_752_000 - 9 * 3600 + 4 * 3600 + 24 * 60),
            "日付を挟んだ `Try again at Aug 28th, 2026 4:24 AM` が読めていない"
        );
    }

    #[test]
    fn issue985_構造化ソースの解除時刻が画面のパースより優先される() {
        // rollout の `resets_at`（epoch）は書式にもタイムゾーンにも依存しない
        let exact = 1_786_752_000 - 9 * 3600 + 4 * 3600 + 24 * 60 + 37;
        let hint = LimitHint::from_codex(&codex_limits(Some(exact)));
        let stop = detect_limit_stop_with(&screen(CODEX_LIMIT_DATED), OBSERVED, JST, Some(&hint))
            .expect("検知される");
        assert_eq!(stop.reset_at, Some(exact), "構造化ソースの epoch を採る");
    }

    #[test]
    fn issue985_構造化ソースだけでは停止と判定しない() {
        // 上限の根拠は画面のまま（#813 の安全条件を 1 か所で守る）。
        // 通常の idle 画面に手がかりを添えても発動しない
        let hint = LimitHint::from_codex(&codex_limits(Some(OBSERVED + 3600)));
        assert!(
            detect_limit_stop_with(&screen(NORMAL_IDLE), OBSERVED, JST, Some(&hint)).is_none(),
            "画面が上限でないのに手がかりだけで自動復帰が発動しうる"
        );
    }

    #[test]
    fn issue985_上限でない構造化ソースは解除時刻を上書きしない() {
        // まだ上限に当たっていない（4% / 1%）なら `reset_at()` は None なので、
        // 画面から読めた時刻がそのまま残る
        let rl = crate::codex_session::RateLimits {
            primary: Some(crate::codex_session::RateWindow {
                used_percent: 4,
                window_minutes: 300,
                resets_at: Some(1_787_840_583),
            }),
            secondary: None,
            plan_type: Some("plus".into()),
            reached: None,
        };
        let hint = LimitHint::from_codex(&rl);
        let stop = detect_limit_stop_with(&screen(CODEX_LIMIT_DATED), OBSERVED, JST, Some(&hint))
            .expect("検知される");
        assert_eq!(
            stop.reset_at,
            Some(1_786_752_000 - 9 * 3600 + 4 * 3600 + 24 * 60),
            "上限でない枠の resets_at で復帰予定を書き換えてはいけない"
        );
    }

    #[test]
    fn issue985_codexの接近ダイアログは現状維持で応答できる() {
        let stop =
            detect_limit_stop(&screen(CODEX_APPROACHING), OBSERVED, JST).expect("検知される");
        assert_eq!(stop.kind, LimitStopKind::Dialog);
        let dialog =
            crate::claude_tui::detect_choice_dialog(&screen(CODEX_APPROACHING)).expect("検知");
        let options: Vec<(Option<u32>, String)> = dialog
            .options
            .iter()
            .map(|o| (o.number, o.label.clone()))
            .collect();
        assert_eq!(
            tako_core::limit_resume::safe_choice(&options),
            Some((2, "Keep current model")),
            "モデル切替（課金に効く）ではなく現状維持を選ぶ"
        );
    }

    /// #985 受け入れ条件 4 の回帰: codex の `/usage` ダイアログでは**何も選ばない**
    #[test]
    fn issue985_codexのusageダイアログでは何も選ばない() {
        let dialog =
            crate::claude_tui::detect_choice_dialog(&screen(CODEX_USAGE_MENU)).expect("検知");
        let options: Vec<(Option<u32>, String)> = dialog
            .options
            .iter()
            .map(|o| (o.number, o.label.clone()))
            .collect();
        assert_eq!(
            tako_core::limit_resume::safe_choice(&options),
            None,
            "在庫のあるリセットを勝手に引き換えてはいけない"
        );
        // そもそも自動復帰の対象にもしない（上限で「止まって」いる画面ではない）
        assert!(detect_limit_stop(&screen(CODEX_USAGE_MENU), OBSERVED, JST).is_none());
    }
}

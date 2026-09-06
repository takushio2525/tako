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
//! 責務（自動復帰は上限由来の停止に限る、という安全条件を 1 か所で守る）。
//! **時間では解けない利用阻害（`WorkerErrorKind::EntitlementBlocked`。#1106）でも None**
//! —— 座席種別や管理者による無効化は待っても解けないので、ナッジを撃つと
//! 試行上限まで空撃ちするだけになる（報告は watch / supervisor の担当）

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

/// 画面のどこかにあるリセット時刻表記を拾う（下の行ほど新しいので末尾から見る）。
///
/// 物理行で読めなければ**折り返しを結合した論理行**でもう一度読む（#1123）。
/// 狭いペインでは `resets Sep 8,` と `3:05pm` のように日付と時刻が別の行へ割れ、
/// 日付つき表記（#1096）が「読めない」= 不明へ落ちてしまう
fn reset_at_from_lines(lines: &[String], observed_at: i64, tz_offset: i32) -> Option<i64> {
    lines
        .iter()
        .rev()
        .find_map(|l| parse_reset_at(l, observed_at, tz_offset))
        .or_else(|| {
            tako_core::limit_resume::unwrapped_tail(lines, RESET_SCAN_LINES)
                .iter()
                .find_map(|l| parse_reset_at(l, observed_at, tz_offset))
        })
}

/// 折り返しを結合してリセット時刻を探すときの走査範囲（画面末尾からの論理行数。#1123）。
/// 物理行の走査は画面全体を見るが、結合後はまとめて見に行くので上限を置く
const RESET_SCAN_LINES: usize = 40;

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

    // --- #1096: `You're out of …` テンプレートと日付つき解除時刻 ---

    /// **実採取**（claude 2.1.258 のバイナリ内の組み立て）。
    /// 組織のクレジットが尽きた形で、`overageStatus === "rejected"` +
    /// `overageDisabledReason === "out_of_credits"` の分岐が作る:
    /// `` `You're out of usage credits${j}${W}` ``（`j` = ` · resets <時刻>`、
    /// `W` = ` · progress saved`）。**`limit` という語が無い**ので #1093 の規則
    /// （`hit your` + `limit`）では原理的に当たらなかった
    const OUT_OF_CREDITS_IDLE: &str = r#"⏺ 実装を進めます

  ⎿  You're out of usage credits · resets 7:50pm (Asia/Tokyo) · progress saved

────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────
  ⏵⏵ accept edits on
  tako
  main
  ~/dev/tako
  ⏸ 待機中 · ? for shortcuts"#;

    #[test]
    fn issue1096_クレジット枯渇の画面を解除時刻つきで検知する() {
        let stop =
            detect_limit_stop(&screen(OUT_OF_CREDITS_IDLE), OBSERVED, JST).expect("検知される");
        assert_eq!(stop.kind, LimitStopKind::Idle);
        assert!(
            stop.message.contains("out of usage credits"),
            "検知の根拠が見出し行になっていない: {}",
            stop.message
        );
        assert_eq!(
            stop.reset_at,
            Some(1_786_752_000 - 9 * 3600 + 19 * 3600 + 50 * 60),
            "`· resets 7:50pm (Asia/Tokyo)` から解除時刻が読めていない"
        );
    }

    #[test]
    fn issue1096_組織のクレジット枯渇も停止として検知する() {
        // 解除時刻を持たない形（管理者へ依頼 / 入金の案内）。時刻不明は core 側の猶予へ
        for line in [
            "Your org is out of usage · add funds to continue",
            "Your org is out of usage · contact your admin",
        ] {
            let src = OUT_OF_CREDITS_IDLE.replace(
                "You're out of usage credits · resets 7:50pm (Asia/Tokyo) · progress saved",
                line,
            );
            let stop = detect_limit_stop(&screen(&src), OBSERVED, JST)
                .unwrap_or_else(|| panic!("検知されない: {line}"));
            assert_eq!(stop.kind, LimitStopKind::Idle);
            assert_eq!(stop.reset_at, None, "{line} に時刻は書かれていない");
        }
    }

    #[test]
    fn issue1096_動詞がreachedの見出しも検知する() {
        // `dCt` の 2 番目 = `You've reached your`。#1093 は `hit` 決め打ちだった
        let src = OUT_OF_CREDITS_IDLE.replace(
            "You're out of usage credits · resets 7:50pm (Asia/Tokyo) · progress saved",
            "You've reached your Fable limit. Switch to another model to continue.",
        );
        let stop = detect_limit_stop(&screen(&src), OBSERVED, JST).expect("検知される");
        assert_eq!(stop.kind, LimitStopKind::Idle);
    }

    #[test]
    fn issue1096_週枠の日付つき解除時刻を丸めない() {
        // 週枠は最大 7 日先なので日付つきが通常形。#1096 前は「次に来る同じ時刻」へ
        // 丸まって、解除の数日前からナッジを撃ち始めていた
        let src = OUT_OF_CREDITS_IDLE.replace(
            "You're out of usage credits · resets 7:50pm (Asia/Tokyo) · progress saved",
            "You've hit your weekly limit · resets Aug 22, 9:15am (Asia/Tokyo)",
        );
        let stop = detect_limit_stop(&screen(&src), OBSERVED, JST).expect("検知される");
        let reset = stop.reset_at.expect("解除時刻が読める");
        // 観測は 2026-08-15 00:30 JST。7 日後の 09:15 になっているか
        assert_eq!(
            (reset - OBSERVED) / 86_400,
            7,
            "日付つきの解除時刻が 24 時間以内へ丸まっている"
        );
    }

    #[test]
    fn issue1106_時間で解けない阻害では自動復帰を発動させない() {
        // #1106 の新種別（`WorkerErrorKind::EntitlementBlocked`）。
        // ここで `Some` を返すと「解除まで待って続行ナッジ」を撃つことになるが、
        // 待っても解けないので **max_attempts 回撃って諦めるだけ**になる。
        // 停止として報告するのは supervisor / watch 側（`WORKER_ERROR`）の仕事
        for line in [
            "Your seat type doesn't include usage credits",
            "Your seat type doesn't include usage",
            "Your seat type doesn't include extra usage",
            "Your usage allocation has been disabled by your admin",
            "Your group's usage limit is set to $0",
            "Fable 5 requires usage credits",
            "You're out of extra usage",
            "This service is disabled for your org",
        ] {
            let src = OUT_OF_CREDITS_IDLE.replace(
                "You're out of usage credits · resets 7:50pm (Asia/Tokyo) · progress saved",
                line,
            );
            assert!(
                detect_limit_stop(&screen(&src), OBSERVED, JST).is_none(),
                "時間で解けない阻害で自動復帰が発動しうる状態になっている: {line}"
            );
            // 一方で「止まっている」ことは新種別として検知できている
            let (kind, detail) = detect_worker_error(&screen(&src).join("\n"))
                .unwrap_or_else(|| panic!("停止として検知されない: {line}"));
            assert_eq!(kind, WorkerErrorKind::EntitlementBlocked, "{line}");
            assert!(
                detail.contains(line),
                "検知の根拠が見出し行になっていない: {detail}"
            );
        }
    }

    #[test]
    fn issue1107_codexとagyの阻害でも自動復帰を発動させない() {
        // **`You've reached your workspace credit limit` は #1107 前は
        // `is_limit_exhausted_line` に当たっていた** = 自動復帰が「解除まで待つ」で
        // 撃ち始め、解けない上限に対して試行上限まで空撃ちしていた
        for line in [
            "You're out of credits.",
            "Your workspace is out of credits. Ask your workspace owner to refill in order to continue.",
            "You hit your spend cap set in your workspace. Increase your spend cap to continue.",
            "You've reached your workspace credit limit",
            "AI: Out of credits",
            "No license available for this project and location. Contact your administrator to setup Gemini Enterprise for this project.",
        ] {
            let src = OUT_OF_CREDITS_IDLE.replace(
                "You're out of usage credits · resets 7:50pm (Asia/Tokyo) · progress saved",
                line,
            );
            assert!(
                detect_limit_stop(&screen(&src), OBSERVED, JST).is_none(),
                "時間で解けない阻害で自動復帰が発動しうる状態になっている: {line}"
            );
            let (kind, detail) = detect_worker_error(&screen(&src).join("\n"))
                .unwrap_or_else(|| panic!("停止として検知されない: {line}"));
            assert_eq!(kind, WorkerErrorKind::EntitlementBlocked, "{line}");
            assert!(
                detail.contains(line),
                "検知の根拠が見出し行になっていない: {detail}"
            );
        }
    }

    #[test]
    fn issue1096_接近の警告では自動復帰を発動させない() {
        // claude 自身が別リスト（`fCt`）に分けている警告。まだ止まっていない
        for line in [
            "You're close to your usage credit limit",
            "You've used 75% of your weekly limit",
            "You're now using usage credits · Your weekly limit resets 7:50pm",
        ] {
            let src = OUT_OF_CREDITS_IDLE.replace(
                "You're out of usage credits · resets 7:50pm (Asia/Tokyo) · progress saved",
                line,
            );
            assert!(
                detect_limit_stop(&screen(&src), OBSERVED, JST).is_none(),
                "警告 / 情報で自動復帰が発動しうる状態になっている: {line}"
            );
        }
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

    /// fixture の `Try again at Aug 28th, 2026 4:24 AM` を JST で解いた絶対時刻。
    ///
    /// **#1096 前の期待値は `OBSERVED` と同じ日（2026-08-15）の 04:24 だった** ——
    /// つまり #985 のテストが「日付を読まず次に来る 4:24 へ丸める」実装を
    /// **13 日早い値のまま固定していた**（`MAX_PARSED_WAIT_SECS` = 24 時間で
    /// 打ち切る設計だったので、日付つきの表記は構造的に丸まっていた）。
    /// 早撃ちの直接証拠なので、直した値との差をテスト側でも検算する
    const CODEX_RESET_AT: i64 = 1_787_858_640;

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
            Some(CODEX_RESET_AT),
            "日付を挟んだ `Try again at Aug 28th, 2026 4:24 AM` が読めていない"
        );
        // 検算: fixture の日付は観測日（2026-08-15）の 13 日後
        assert_eq!(
            (CODEX_RESET_AT - (1_786_752_000 - 9 * 3600)) / 86_400,
            13,
            "期待値が fixture の日付と合っていない"
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
            Some(CODEX_RESET_AT),
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

    /// #1123: **幅 25 桁**で上限に当たったペインの実採取（2026-09-04。worker 4 体が
    /// 解除後 7.5 時間止まったままだった画面）。claude が自分で折り返しているので
    /// どの物理行も #1093 の規則に当たらない。フッターに `5h` / `7d` が無いのも症状どおり
    const WRAPPED_SESSION_LIMIT_IDLE: &str = r#"⏺ 実装を進めます

  ⎿  You've hit your
     session limit ·
     resets 5:50am
     (Asia/Tokyo)
     /usage-credits to
     request more usage
     from your admin.

─────────────────────────
❯
─────────────────────────
  ⏵⏵ accept edits on
  tako
  main
  ~/dev/tako
  ⏸ 待機中 · ? for
  shortcuts"#;

    #[test]
    fn issue1123_折り返した見出しでも上限停止として検知する() {
        let stop = detect_limit_stop(&screen(WRAPPED_SESSION_LIMIT_IDLE), OBSERVED, JST)
            .expect("折り返した見出しが検知されない（#1123 の実害）");
        assert_eq!(
            stop.kind,
            LimitStopKind::Idle,
            "ダイアログ無しなので idle 型"
        );
        assert!(
            stop.message.contains("hit your session limit"),
            "検知の根拠が結合後の 1 本になっていない: {}",
            stop.message
        );
        // 解除時刻（`resets 5:50am`）も同じ 1 本から読める。
        // JST 00:30 観測 → その日の 05:50 = 5 時間 20 分先
        assert_eq!(
            stop.reset_at,
            Some(OBSERVED + 5 * 3600 + 20 * 60),
            "折り返した `resets 5:50am` が読めていない"
        );
    }

    #[test]
    fn issue1123_折り返した日付つき解除時刻も読む() {
        // 週枠は最大 7 日先なので日付つきが通常形（#1096）。狭いペインでは
        // 日付と時刻が別の行へ割れ、「日付は在るが読めない」= 不明へ落ちていた
        let src = WRAPPED_SESSION_LIMIT_IDLE
            .replace("     session limit ·\n", "     weekly limit ·\n")
            .replace(
                "     resets 5:50am\n     (Asia/Tokyo)\n",
                "     resets Aug 22,\n     9:15am (Asia/Tokyo)\n",
            );
        let stop = detect_limit_stop(&screen(&src), OBSERVED, JST).expect("検知される");
        let at = stop.reset_at.expect("日付つきの解除時刻が読めていない");
        assert!(
            at > OBSERVED + 6 * 86_400,
            "24 時間以内へ丸められている: {at} (observed={OBSERVED})"
        );
    }

    #[test]
    fn issue1123_折り返しても上限でない画面は検知しない() {
        // 25 桁に折り返した通常 idle / permission / API エラー / 警告。
        // 結合は候補を増やすだけなので、これらが上限になってはいけない
        for src in [
            "⏺ 実装が完了しました。\n  テストは全て緑です。\n\n─────────────────────────\n❯\n─────────────────────────",
            "  ⎿  You've used 90%\n     of your session\n     limit\n\n─────────────────────────\n❯\n─────────────────────────",
            "  ⎿  API Error:\n     Connection closed\n     mid-response.\n\n─────────────────────────\n❯\n─────────────────────────",
            "  ⎿  Opus limit\n     reached, now using\n     Sonnet\n\n─────────────────────────\n❯\n─────────────────────────",
        ] {
            assert!(
                detect_limit_stop(&screen(src), OBSERVED, JST).is_none(),
                "上限ではない画面を検知した: {src}"
            );
        }
    }

    #[test]
    fn issue1123_折り返した阻害は上限にならない() {
        // 時間では解けない阻害（#1106）は結合後も阻害のまま = 自動復帰は発動しない
        let src = WRAPPED_SESSION_LIMIT_IDLE.replace(
            "  ⎿  You've hit your\n     session limit ·\n     resets 5:50am\n     (Asia/Tokyo)\n     /usage-credits to\n     request more usage\n     from your admin.\n",
            "  ⎿  Your seat type\n     doesn't include\n     usage credits\n",
        );
        assert!(
            detect_limit_stop(&screen(&src), OBSERVED, JST).is_none(),
            "待っても解けない阻害で自動復帰を発動させてはいけない"
        );
        // ただし worker の異常としては検知される（idle = 作業完了に見せない）
        let joined: String = screen(&src).join("\n");
        assert!(matches!(
            crate::orchestrator::wait::detect_worker_error(&joined),
            Some((
                crate::orchestrator::wait::WorkerErrorKind::EntitlementBlocked,
                _
            ))
        ));
    }
}

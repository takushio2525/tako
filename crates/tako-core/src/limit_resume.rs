//! 利用上限（5h / 週次）後のペイン単位の自動復帰（Issue #813）の純粋ロジック。
//!
//! エージェントが上限に当たると、リセットまで止まったまま人間の再開操作を待つ。
//! 自律で回したい長時間タスクのために、**ペインごとにオプトイン**しておくと
//! tako がリセット時刻を過ぎたのを見て自動で作業を再開させる。
//!
//! # ここに置くもの / 置かないもの
//!
//! - 置く: リセット時刻のパース・発動判断・安全な選択肢の選別（すべて純関数）
//! - 置かない: 画面の採取・ダイアログの種別分類（文言依存なので
//!   `tako-control::claude_tui`）・実際のキー送出（`tako-control::dispatch`）
//!
//! # 安全側に倒す設計（Issue #813 の安全条件）
//!
//! - 既定 OFF。有効なペインが 1 つも無ければ判断そのものを走らせない（2 秒 tick に
//!   重い処理を足さない。#772 / #779 の教訓）
//! - **上限由来の停止だけ**を対象にする。permission ダイアログ・API エラー・
//!   通常の idle では発動しない（種別は呼び出し側が `LimitStop` に詰めて渡す）
//! - 画面が動いている間は触らない（生成中の可能性がある。#572 の教訓で
//!   「busy の文言」ではなく**画面が変化していないこと**を条件にする）
//! - 人間の下書きが入力欄にあれば上書きしない
//! - 1 エピソード（= 上限で止まってから復帰するまで）あたりの試行回数を
//!   [`MAX_ATTEMPTS`] で打ち切る（無限リトライしない）
//! - 選択肢は許可リストで選び、課金・モデル変更を伴うラベルは拒否リストで**構造的に**弾く

use crate::i18n::{lang, Lang};

// --- 定数（すべてテストで固定する） ---

/// リセット時刻を過ぎてから実際に動くまでの安全マージン。
/// TUI 側の解除反映がわずかに遅れることがあるので数分待つ
pub const SAFETY_MARGIN_SECS: i64 = 120;

/// リセット時刻を解決できなかったときの初回試行までの猶予（上限停止の初観測から）
pub const UNKNOWN_RESET_FALLBACK_SECS: i64 = 900;

/// 失敗後の再試行間隔
pub const RETRY_INTERVAL_SECS: i64 = 300;

/// 1 エピソードあたりの試行上限（超えたら黙って諦め、状態は照会できる形で残す）
pub const MAX_ATTEMPTS: u32 = 3;

/// 画面が変化しなくなってから発動するまでの確認時間。
/// 「生成中ではない」ことを文言ではなく**画面が動いていないこと**で確かめる
pub const STABLE_SECS: i64 = 10;

/// パースしたリセット時刻として受け入れる最大の待ち時間。
/// これを超える値は誤パースとみなして「不明」に落とす（時刻表記は 24 時間で 1 周する）
pub const MAX_PARSED_WAIT_SECS: i64 = 24 * 3600;

// --- 上限による停止 ---

/// 上限で止まっている画面の型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitStopKind {
    /// 上限の対処ダイアログが出ている（#748 の `DialogKind::UsageLimit`）
    Dialog,
    /// ダイアログは無く、上限メッセージを出したまま止まっている（#157 の `usage_limit`）
    Idle,
}

impl LimitStopKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dialog => "dialog",
            Self::Idle => "idle",
        }
    }
}

/// 上限による停止の観測結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitStop {
    pub kind: LimitStopKind,
    /// 検知の根拠になった画面上の 1 行（監査ログ・状態照会に載せる）
    pub message: String,
    /// 解決できたリセット時刻（unix 秒）。None = 不明
    pub reset_at: Option<i64>,
}

// --- 発動判断 ---

/// 判断の材料（すべて呼び出し側が観測して詰める。ここでは I/O をしない）
#[derive(Debug, Clone)]
pub struct ResumeInput<'a> {
    /// ペイン属性（既定 false）
    pub enabled: bool,
    /// 上限による停止（None = 上限ではない = 発動しない）
    pub stop: Option<&'a LimitStop>,
    /// 現在時刻（unix 秒。テストで注入する）
    pub now: i64,
    /// このエピソードで最初に上限停止を観測した時刻（unix 秒）
    pub first_seen: i64,
    /// 画面が変化しなくなった時刻（unix 秒）。None = まだ動いている
    pub stable_since: Option<i64>,
    /// このエピソードでの試行回数
    pub attempts: u32,
    /// 直近の試行時刻（unix 秒）
    pub last_attempt: Option<i64>,
    /// 入力欄に人間の下書きがあるか（`input_status` が user / mixed）
    pub user_draft: bool,
}

/// 実際に行う復帰動作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAction {
    /// 上限ダイアログへ安全な選択肢で応答する
    RespondDialog,
    /// 継続ナッジを送達する
    Nudge,
}

/// 発動しない理由（状態照会・監査ログにそのまま載せる）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    /// ペインで自動復帰が無効
    Disabled,
    /// 上限による停止ではない
    NotLimited,
    /// 入力欄に人間の下書きがある
    UserDraft,
    /// 画面がまだ動いている（生成中の可能性）
    ScreenUnstable,
    /// リセット時刻（+ 安全マージン）待ち
    WaitingForReset,
    /// 直近の試行から間隔を空けている
    RetryBackoff,
    /// 試行上限に達した
    AttemptsExhausted,
}

impl HoldReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NotLimited => "not_limited",
            Self::UserDraft => "user_draft",
            Self::ScreenUnstable => "screen_unstable",
            Self::WaitingForReset => "waiting_for_reset",
            Self::RetryBackoff => "retry_backoff",
            Self::AttemptsExhausted => "attempts_exhausted",
        }
    }
}

/// 判断の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDecision {
    /// 何もしない（理由つき。`remaining_secs` は待ち系の理由でだけ意味を持つ）
    Hold {
        reason: HoldReason,
        remaining_secs: i64,
    },
    /// 復帰動作を行う
    Act(ResumeAction),
}

impl ResumeDecision {
    pub fn action(self) -> Option<ResumeAction> {
        match self {
            Self::Act(a) => Some(a),
            Self::Hold { .. } => None,
        }
    }

    fn hold(reason: HoldReason) -> Self {
        Self::Hold {
            reason,
            remaining_secs: 0,
        }
    }

    fn wait(reason: HoldReason, remaining_secs: i64) -> Self {
        Self::Hold {
            reason,
            remaining_secs: remaining_secs.max(0),
        }
    }
}

/// このエピソードで最初に復帰を試みてよくなる時刻（unix 秒）。
///
/// リセット時刻が分かっていればそこへ安全マージンを足し、分からなければ
/// 初観測からの固定猶予に落とす（諦めるのではなく、遅らせてから 1 度だけ試す）
pub fn due_at(stop: &LimitStop, first_seen: i64) -> i64 {
    match stop.reset_at {
        Some(reset) => reset + SAFETY_MARGIN_SECS,
        None => first_seen + UNKNOWN_RESET_FALLBACK_SECS,
    }
}

/// 自動復帰を発動するかを決める（純関数）。
///
/// 判定順は「安いもの・確実に止めるものから」。無効なペインでは最初の 1 行で抜けるので、
/// 2 秒 tick に画面走査がぶら下がらない
pub fn decide(input: &ResumeInput) -> ResumeDecision {
    if !input.enabled {
        return ResumeDecision::hold(HoldReason::Disabled);
    }
    let Some(stop) = input.stop else {
        return ResumeDecision::hold(HoldReason::NotLimited);
    };
    // 人間が打ちかけた指示を復帰動作で潰さない（安全条件）
    if input.user_draft {
        return ResumeDecision::hold(HoldReason::UserDraft);
    }
    if input.attempts >= MAX_ATTEMPTS {
        return ResumeDecision::hold(HoldReason::AttemptsExhausted);
    }
    // 画面が動いている = まだ生成中かもしれない。文言ではなく変化の有無で見る（#572）
    let Some(stable_since) = input.stable_since else {
        return ResumeDecision::wait(HoldReason::ScreenUnstable, STABLE_SECS);
    };
    let stable_for = input.now - stable_since;
    if stable_for < STABLE_SECS {
        return ResumeDecision::wait(HoldReason::ScreenUnstable, STABLE_SECS - stable_for);
    }
    let due = due_at(stop, input.first_seen);
    if input.now < due {
        return ResumeDecision::wait(HoldReason::WaitingForReset, due - input.now);
    }
    if let Some(last) = input.last_attempt {
        let since = input.now - last;
        if since < RETRY_INTERVAL_SECS {
            return ResumeDecision::wait(HoldReason::RetryBackoff, RETRY_INTERVAL_SECS - since);
        }
    }
    ResumeDecision::Act(match stop.kind {
        LimitStopKind::Dialog => ResumeAction::RespondDialog,
        LimitStopKind::Idle => ResumeAction::Nudge,
    })
}

// --- 安全な選択肢の選別（#748 の許可リスト + #813 の拒否リスト） ---

/// 選んでよい選択肢のラベル断片（小文字で比較）。
///
/// 実採取ダイアログ（claude v2.1.220 の limit 対処 / codex のレート制限）に基づく。
/// 優先順は「解除まで待つ > 現状維持 > 停止」。
/// **実採取のラベルだけを載せる**（推測で増やさない）。未知の言い回しは
/// 「安全な選択肢が無い」= 何もしない、に落ちるほうが安全側
pub const SAFE_CHOICE_ALLOW: [&str; 2] = ["wait for limit to reset", "keep current model"];

/// **絶対に選ばない**ラベル断片（小文字で比較）。
///
/// 許可リストに当たっていても、これらを含むラベルは弾く。上限ダイアログの
/// 他の選択肢は課金プラン変更・従量課金・モデル切替を伴うので、
/// 自動操作で確定させてはいけない（実採取: 「Upgrade to Max 20x for higher session
/// limits every month」「Continue with usage credits」「Switch to gpt-…」）
pub const SAFE_CHOICE_DENY: [&str; 12] = [
    "upgrade",
    "credit",
    "billing",
    "purchase",
    "buy ",
    "subscribe",
    "subscription",
    "switch to",
    "change model",
    "extra usage",
    "pay ",
    "$",
];

/// ラベルが自動確定してよいものか（許可リストに当たり、拒否リストに当たらない）
pub fn is_safe_choice_label(label: &str) -> bool {
    let l = label.to_lowercase();
    SAFE_CHOICE_ALLOW.iter().any(|n| l.contains(n))
        && !SAFE_CHOICE_DENY.iter().any(|n| l.contains(n))
}

/// 「停止」だけの選択肢か（`stop` 単独 / `stop …`）。
/// 待つ選択肢が無いダイアログでの最後の逃げ道に使う
fn is_stop_label(label: &str) -> bool {
    let l = label.to_lowercase();
    if SAFE_CHOICE_DENY.iter().any(|n| l.contains(n)) {
        return false;
    }
    l == "stop" || l.starts_with("stop ")
}

/// 上限ダイアログから自動確定してよい選択肢を選ぶ。
///
/// 返り値は `(表示番号, ラベル)`。**番号ではなくラベルで選ぶ**のが要点で、
/// 選択肢の並びが版で変わっても課金系を掴まない。安全なものが無ければ `None`
/// （呼び出し側は自動操作をやめて報告だけに落ちる）
pub fn safe_choice(options: &[(Option<u32>, String)]) -> Option<(u32, &str)> {
    let numbered = |i: usize, n: Option<u32>| n.unwrap_or((i + 1) as u32);
    for needle in SAFE_CHOICE_ALLOW {
        if let Some((i, (n, label))) = options
            .iter()
            .enumerate()
            .find(|(_, (_, l))| l.to_lowercase().contains(needle) && is_safe_choice_label(l))
        {
            return Some((numbered(i, *n), label.as_str()));
        }
    }
    options
        .iter()
        .enumerate()
        .find(|(_, (_, l))| is_stop_label(l))
        .map(|(i, (n, label))| (numbered(i, *n), label.as_str()))
}

// --- リセット時刻のパース ---

/// リセット時刻が書かれている箇所の目印（小文字で検索。実採取の文言由来）。
///
/// - claude: 「Your limit will reset at 3am.」「Your limit will reset at 3:00 AM (JST)」
/// - claude: 「5-hour limit reached ∙ resets 3am」
/// - codex: 「You've hit your usage limit. ... try again at 4:24 AM.」
const RESET_ANCHORS: [&str; 4] = ["will reset at ", "try again at ", "resets at ", "resets "];

/// 画面テキストからリセット時刻（unix 秒）を解決する。
///
/// `reference` は「そのメッセージを観測した時刻」（unix 秒）。時刻表記は日付を持たない
/// ので、**観測時刻から見て次に来る同じ時刻**として解釈する。`tz_offset` は
/// ローカルタイムの UTC からのずれ（秒。日本なら +32400）
pub fn parse_reset_at(text: &str, reference: i64, tz_offset: i32) -> Option<i64> {
    let lower = text.to_lowercase();
    let tod = RESET_ANCHORS.iter().find_map(|anchor| {
        let pos = lower.find(anchor)?;
        parse_time_of_day(&lower[pos + anchor.len()..])
    })?;
    let local_ref = reference + i64::from(tz_offset);
    let day_start = local_ref - local_ref.rem_euclid(86_400);
    let mut target = day_start + tod;
    if target <= local_ref {
        target += 86_400;
    }
    let wait = target - local_ref;
    if wait <= 0 || wait > MAX_PARSED_WAIT_SECS {
        return None;
    }
    Some(target - i64::from(tz_offset))
}

/// 「3am」「3:00 am」「15:00」「4:24 AM」を 0:00 からの秒数に変換する。
/// 先頭が数字でなければ None（誤パースを避けるため前方一致でしか読まない）
fn parse_time_of_day(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let mut chars = s.chars().peekable();
    let mut digits = String::new();
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        digits.push(chars.next()?);
    }
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let mut hour: i64 = digits.parse().ok()?;
    let mut minute: i64 = 0;
    if chars.peek() == Some(&':') {
        chars.next();
        let mut min_digits = String::new();
        while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
            min_digits.push(chars.next()?);
        }
        if min_digits.len() != 2 {
            return None;
        }
        minute = min_digits.parse().ok()?;
    }
    let rest: String = chars.collect();
    let rest = rest.trim_start();
    if rest.starts_with("pm") {
        if hour > 12 {
            return None;
        }
        if hour < 12 {
            hour += 12;
        }
    } else if rest.starts_with("am") {
        if hour > 12 {
            return None;
        }
        if hour == 12 {
            hour = 0;
        }
    } else if hour > 23 {
        // 24 時間表記として読めない値（AM/PM も無い）は誤パース
        return None;
    }
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 3_600 + minute * 60)
}

/// ローカルタイムの UTC からのずれ（秒）。夏時間も含めて「いまの」値を返す。
///
/// 時刻表記のパースだけに使う。純関数側（[`parse_reset_at`]）は
/// 引数で受け取るので、テストは実機のタイムゾーンに依存しない。
/// OS 依存の取得は `platform::clock` の境界の内側（#467 の原則）
pub fn local_utc_offset() -> i32 {
    crate::platform::clock::local_utc_offset()
}

/// 現在時刻（unix 秒）
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- 継続ナッジの文面 ---

/// idle 型（ダイアログ無し）で送る継続ナッジ。
/// #749 と同じく【tako 自動通知】で始め、AI 側が「人間の指示」と取り違えないようにする
pub fn nudge_prompt() -> String {
    nudge_prompt_in(lang())
}

/// 言語を明示してのナッジ文面（#608: テストは言語グローバルに触らない）
pub fn nudge_prompt_in(lang: Lang) -> String {
    match lang {
        Lang::Ja => "【tako 自動通知】利用上限が解除されました。\
             中断していた作業をそのまま続けてください。\
             どこまで進んでいたか分からなければ、まず直近の状況を確認してから再開してください。"
            .to_string(),
        Lang::En => "[tako auto-notice] The usage limit has reset. \
             Continue the work you were interrupted on. \
             If you are unsure where you left off, check your recent state first, then resume."
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(kind: LimitStopKind, reset_at: Option<i64>) -> LimitStop {
        LimitStop {
            kind,
            message: "Claude usage limit reached. Your limit will reset at 3am.".to_string(),
            reset_at,
        }
    }

    fn input<'a>(enabled: bool, s: Option<&'a LimitStop>, now: i64) -> ResumeInput<'a> {
        ResumeInput {
            enabled,
            stop: s,
            now,
            first_seen: 0,
            stable_since: Some(0),
            attempts: 0,
            last_attempt: None,
            user_draft: false,
        }
    }

    // --- 発動判断 ---

    #[test]
    fn 無効なペインでは何があっても発動しない() {
        let s = stop(LimitStopKind::Dialog, Some(0));
        let mut i = input(false, Some(&s), 100_000);
        assert_eq!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::Disabled,
                remaining_secs: 0
            }
        );
        // 有効にすれば同じ材料で発動する（= 判定を止めていたのは enabled だけ）
        i.enabled = true;
        assert_eq!(decide(&i).action(), Some(ResumeAction::RespondDialog));
    }

    #[test]
    fn 上限以外の停止では発動しない() {
        let i = input(true, None, 100_000);
        assert_eq!(decide(&i).action(), None);
        assert!(matches!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::NotLimited,
                ..
            }
        ));
    }

    #[test]
    fn リセット時刻前は待ち残り秒つきで待つ() {
        let s = stop(LimitStopKind::Idle, Some(1_000));
        let i = input(true, Some(&s), 900);
        assert_eq!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::WaitingForReset,
                // 1000 + マージン 120 - 900
                remaining_secs: 220,
            }
        );
        // マージンを過ぎたら発動する
        let i2 = input(true, Some(&s), 1_000 + SAFETY_MARGIN_SECS);
        assert_eq!(decide(&i2).action(), Some(ResumeAction::Nudge));
    }

    #[test]
    fn リセット時刻不明なら初観測からの猶予後に一度だけ試す() {
        let s = stop(LimitStopKind::Idle, None);
        let mut i = input(true, Some(&s), UNKNOWN_RESET_FALLBACK_SECS - 1);
        assert!(matches!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::WaitingForReset,
                ..
            }
        ));
        i.now = UNKNOWN_RESET_FALLBACK_SECS;
        assert_eq!(decide(&i).action(), Some(ResumeAction::Nudge));
    }

    #[test]
    fn 人間の下書きがあれば上書きしない() {
        let s = stop(LimitStopKind::Idle, Some(0));
        let mut i = input(true, Some(&s), 100_000);
        i.user_draft = true;
        assert!(matches!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::UserDraft,
                ..
            }
        ));
    }

    #[test]
    fn 画面が動いている間は発動しない() {
        let s = stop(LimitStopKind::Dialog, Some(0));
        let mut i = input(true, Some(&s), 100_000);
        i.stable_since = None;
        assert!(matches!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::ScreenUnstable,
                ..
            }
        ));
        // 静止して間もないときも待つ
        i.stable_since = Some(100_000 - (STABLE_SECS - 1));
        assert!(matches!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::ScreenUnstable,
                remaining_secs: 1,
            }
        ));
        i.stable_since = Some(100_000 - STABLE_SECS);
        assert!(decide(&i).action().is_some());
    }

    #[test]
    fn 試行は間隔を空けて上限で打ち切る() {
        let s = stop(LimitStopKind::Idle, Some(0));
        let mut i = input(true, Some(&s), 100_000);
        i.attempts = 1;
        i.last_attempt = Some(100_000 - (RETRY_INTERVAL_SECS - 1));
        assert!(matches!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::RetryBackoff,
                remaining_secs: 1,
            }
        ));
        i.last_attempt = Some(100_000 - RETRY_INTERVAL_SECS);
        assert_eq!(decide(&i).action(), Some(ResumeAction::Nudge));
        // 上限に達したら間隔を空けても発動しない
        i.attempts = MAX_ATTEMPTS;
        assert!(matches!(
            decide(&i),
            ResumeDecision::Hold {
                reason: HoldReason::AttemptsExhausted,
                ..
            }
        ));
    }

    #[test]
    fn 停止の型で動作が分かれる() {
        let dialog = stop(LimitStopKind::Dialog, Some(0));
        let idle = stop(LimitStopKind::Idle, Some(0));
        assert_eq!(
            decide(&input(true, Some(&dialog), 100_000)).action(),
            Some(ResumeAction::RespondDialog)
        );
        assert_eq!(
            decide(&input(true, Some(&idle), 100_000)).action(),
            Some(ResumeAction::Nudge)
        );
    }

    // --- 安全な選択肢 ---

    fn opts(labels: &[&str]) -> Vec<(Option<u32>, String)> {
        labels
            .iter()
            .enumerate()
            .map(|(i, l)| (Some((i + 1) as u32), l.to_string()))
            .collect()
    }

    #[test]
    fn 実採取のlimitダイアログでは待つ選択肢を選ぶ() {
        // #748 の実採取 fixture と同じ並び
        let o = opts(&[
            "Stop and wait for limit to reset",
            "Upgrade to Max 20x for higher session limits every month",
            "Continue with usage credits",
        ]);
        assert_eq!(
            safe_choice(&o),
            Some((1, "Stop and wait for limit to reset"))
        );
    }

    #[test]
    fn 待つ選択肢が先頭でなくてもラベルで選ぶ() {
        let o = opts(&[
            "Upgrade to Max 20x for higher session limits every month",
            "Continue with usage credits",
            "Wait for limit to reset",
        ]);
        assert_eq!(safe_choice(&o), Some((3, "Wait for limit to reset")));
    }

    #[test]
    fn 課金モデル変更の選択肢は決して選ばない() {
        // 拒否リストの全項目を「もっともらしい選択肢」として並べても選ばれない
        for label in [
            "Upgrade to Max 20x for higher session limits every month",
            "Continue with usage credits",
            "Update billing settings",
            "Purchase additional capacity",
            "Buy more usage",
            "Subscribe to Max",
            "Manage subscription",
            "Switch to gpt-5-codex-mini",
            "Change model to Sonnet",
            "Enable extra usage",
            "Pay as you go",
            "Add $20 of credits",
        ] {
            assert!(
                !is_safe_choice_label(label),
                "自動確定してはいけない選択肢が通った: {label}"
            );
            assert_eq!(
                safe_choice(&opts(&[label])),
                None,
                "危険な選択肢しか無いダイアログでは何も選ばない: {label}"
            );
        }
    }

    #[test]
    fn 待つ選択肢に課金語が混ざっていたら選ばない() {
        // 「待つ」に見えても課金語を含むラベルは弾く（許可リストより拒否リストが強い）
        let o = opts(&["Wait for limit to reset, or upgrade to Max 20x"]);
        assert_eq!(safe_choice(&o), None);
    }

    #[test]
    fn 待つ選択肢が無ければ停止を選び危険な停止は選ばない() {
        assert_eq!(
            safe_choice(&opts(&["Stop", "Continue with usage credits"])),
            Some((1, "Stop"))
        );
        // 「Stop」に見えても課金を伴えば選ばない
        assert_eq!(safe_choice(&opts(&["Stop and buy more credits"])), None);
    }

    #[test]
    fn codexのモデル維持は選べる() {
        let o = opts(&["Switch to gpt-5-codex-mini", "Keep current model"]);
        assert_eq!(safe_choice(&o), Some((2, "Keep current model")));
    }

    #[test]
    fn 番号が画面と食い違っても表示番号を返す() {
        let o = vec![
            (Some(2), "Upgrade to Max 20x".to_string()),
            (Some(3), "Wait for limit to reset".to_string()),
        ];
        assert_eq!(safe_choice(&o), Some((3, "Wait for limit to reset")));
        // 番号なしダイアログでは並び順から補う
        let o2 = vec![
            (None, "Upgrade".to_string()),
            (None, "Wait for limit to reset".to_string()),
        ];
        assert_eq!(safe_choice(&o2), Some((2, "Wait for limit to reset")));
    }

    // --- リセット時刻のパース ---

    /// JST（+9h）。テストは実機のタイムゾーンに依存しない
    const JST: i32 = 9 * 3600;

    /// ちょうど JST の 00:00:00 になる unix 秒（1_786_752_000 は UTC の 00:00:00）
    const REF_MIDNIGHT_JST: i64 = 1_786_752_000 - 9 * 3600;

    #[test]
    fn 実採取の文言からリセット時刻を解決する() {
        // 00:30 JST に「3am」を観測 → 同日 03:00 JST
        let reference = REF_MIDNIGHT_JST + 30 * 60;
        for text in [
            "⎿  Claude usage limit reached. Your limit will reset at 3am.",
            "Your limit will reset at 3:00 AM (JST)",
            "5-hour limit reached ∙ resets 3am",
            "You've hit your usage limit. Please try again at 3:00 AM.",
        ] {
            let at = parse_reset_at(text, reference, JST).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(
                at,
                REF_MIDNIGHT_JST + 3 * 3600,
                "{text} のリセット時刻がずれている"
            );
        }
    }

    #[test]
    fn 観測時刻より前の時刻は翌日として解釈する() {
        // 22:00 JST に「3am」を観測 → 翌日 03:00 JST（= 5 時間後）
        let reference = REF_MIDNIGHT_JST + 22 * 3600;
        let at = parse_reset_at("Your limit will reset at 3am.", reference, JST).expect("解決する");
        assert_eq!(at - reference, 5 * 3600);
    }

    #[test]
    fn 午後表記と24時間表記を解釈する() {
        let reference = REF_MIDNIGHT_JST + 9 * 3600; // 09:00 JST
        let pm = parse_reset_at("resets at 3:30 pm", reference, JST).expect("pm");
        assert_eq!(pm, REF_MIDNIGHT_JST + 15 * 3600 + 30 * 60);
        let h24 = parse_reset_at("resets at 15:30", reference, JST).expect("24h");
        assert_eq!(h24, pm);
        // 12am = 深夜 0 時 / 12pm = 正午
        let midnight = parse_reset_at("resets at 12am", reference, JST).expect("12am");
        assert_eq!(midnight, REF_MIDNIGHT_JST + 86_400);
        let noon = parse_reset_at("resets at 12pm", reference, JST).expect("12pm");
        assert_eq!(noon, REF_MIDNIGHT_JST + 12 * 3600);
    }

    #[test]
    fn タイムゾーンが違えば同じ表記でも別の瞬間になる() {
        let text = "Your limit will reset at 3am.";
        // UTC の 00:30 に観測
        let reference = 1_786_800_000 - 1_786_800_000 % 86_400 + 30 * 60;
        let utc = parse_reset_at(text, reference, 0).expect("utc");
        let jst = parse_reset_at(text, reference, JST).expect("jst");
        assert_ne!(utc, jst, "タイムゾーンを無視して同じ結果になっている");
    }

    #[test]
    fn 時刻の無い文言や壊れた表記は不明として扱う() {
        let reference = REF_MIDNIGHT_JST;
        for text in [
            "Claude usage limit reached.",
            "Your limit will reset soon",
            "resets at later",
            "resets at 99:00",
            "resets at 25:00",
            "resets at 13pm",
            "resets at 3:0",
            "API Error: Connection closed mid-response",
        ] {
            assert_eq!(
                parse_reset_at(text, reference, JST),
                None,
                "{text} を時刻として読んでしまった"
            );
        }
    }

    #[test]
    fn ローカルオフセットは実在の値を返す() {
        // 実機依存なので範囲だけ検査する（UTC-12 〜 UTC+14）
        let off = local_utc_offset();
        assert!(
            (-12 * 3600..=14 * 3600).contains(&off),
            "ありえないオフセット: {off}"
        );
    }

    #[test]
    fn ナッジ文面は自動通知と分かる形で日英ある() {
        assert!(nudge_prompt_in(Lang::Ja).starts_with("【tako 自動通知】"));
        assert!(nudge_prompt_in(Lang::En).starts_with("[tako auto-notice]"));
    }
}

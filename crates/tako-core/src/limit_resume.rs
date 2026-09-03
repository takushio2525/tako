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
///
/// **codex 0.150.1 のバイナリ内文字列で確認した追加分（#985）**。codex の上限まわりは
/// 「待つ」以外の出口しか持たないので、取り違えると人のお金・限りある資源を使う:
/// - 「Request a limit increase from your owner …」= 管理者へ増枠を申請する
/// - 「Your workspace is out of credits. Ask your workspace owner to add more. Notify owner?」
/// - 「Use this reset? / Yes, use reset」= **獲得済みリセットの引き換え**（在庫が減る）
pub const SAFE_CHOICE_DENY: [&str; 17] = [
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
    // --- #985: codex ---
    "use reset",
    "request increase",
    "limit increase",
    "notify owner",
    "add more",
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

// --- 上限に達した見出し行の判定（文言の正本。Issue #1093） ---

/// 上限が尽きたことを告げる見出し行か（claude / codex 共通）。
///
/// **文言の正本はここ 1 箇所**にする。停止判定（`tako-control::orchestrator::wait`）と
/// ステータスバーのメーター（`crate::terminal`）が同じ規則を通るので、
/// 「自動復帰は発動したのにメーターは `--`」のような食い違いが構造的に起きない。
///
/// # なぜ「`hit your` + `limit`」で足りるのか（claude 2.1.258 のバイナリ実測）
///
/// claude の見出しは**1 つのテンプレート**から作られる:
///
/// ```text
/// function oO(e,n,r,o){ … return `You've hit your ${e}${n}${d}` }
/// ```
///
/// `${e}` に入る限度の名前は同バイナリの表と呼び出し側から次のとおりで、
/// **すべて `limit` で終わる**（テンプレートを規則にすれば版の追加に自動で追従する）:
///
/// - `nF` の表: `session limit`（5h）/ `weekly limit`（週）/ `Opus limit` /
///   `Sonnet limit` / `Fable limit` / `usage credit limit`
/// - 支出上限の呼び出し: `individual spend limit` / `monthly spend limit` /
///   `org's monthly spend limit` / `org's monthly usage limit`
/// - 総称の呼び出し: `usage limit` / `limit`
///
/// `${n}` は解除時刻や対処の案内（` · resets 7:50pm (Asia/Tokyo)` /
/// ` · contact your admin to increase it` 等）で、時刻は [`parse_reset_at`] が読む。
/// codex も同じ形（`You've hit your usage limit.`）なので 1 つの規則で両方に効く。
///
/// アポストロフィは**判定に使わない**。同じバイナリの中で ASCII の `'`
/// （`You've hit your …`）と U+2019（`you’re working on …`）が混在しているので、
/// どちらかに寄せた判定は片方を取りこぼす。
///
/// # 旧来の文言も同じ規則の下に置く
///
/// `Claude usage limit reached. Your limit will reset at 3am.`（#157 で実採取）と
/// `5-hour limit reached ∙ resets 3am` も上限の告知なのでここで受ける。
/// ただし **`limit reached, now using …` は除く** —— これは自動モデル切替の告知で、
/// worker は止まらない（#157 の実採取由来の除外条件）。
pub fn is_limit_exhausted_line(line: &str) -> bool {
    // 自動モデル切替の告知は上限「到達」ではあるが停止しない
    if line.contains("limit reached, now using") {
        return false;
    }
    if legacy_session_limit() {
        // #1093 前の規則（`hit your usage limit` 決め打ち）へ戻す A/B 用
        return line.contains("hit your usage limit")
            || line.contains("usage limit reached")
            || (line.contains("limit reached") && line.contains("reset"));
    }
    // テンプレート `You've hit your <限度の名前><理由や解除時刻>`。
    // 限度の名前は `hit your` の**直後**にあり、後ろに続く理由や解除時刻は必ず
    // `·` か `.` で始まる（`… session limit · resets 7:50pm` /
    // `… usage limit. Upgrade to Pro …`）。そこで**名前の区間だけ**を見る。
    // `,` は限度の名前には現れないので、地の文（「… hit your head on this design,
    // the rate limit docs …」）を上限と読まないための追加の境界として足してある。
    // 句読点が来ない病的な行には文字数で歯止めをかける
    if let Some((_, rest)) = line.split_once("hit your") {
        let name = rest.split(['·', '.', ',']).next().unwrap_or(rest);
        let name = name
            .char_indices()
            .nth(LIMIT_NAME_MAX_CHARS)
            .map_or(name, |(i, _)| &name[..i]);
        if name.contains("limit") {
            return true;
        }
    }
    // 旧来の文言
    line.contains("usage limit reached")
        || (line.contains("limit reached") && line.contains("reset"))
}

/// 使用量メーターの枠（ステータスバーの `5h` / `7d`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitWindow {
    /// 5 時間のセッション枠（claude の `five_hour`）
    FiveHour,
    /// 週枠（claude の `seven_day` / `seven_day_opus` / `seven_day_sonnet`）
    Week,
}

/// 上限に達した見出し行から「どの枠が尽きたか」を読む（読めたときだけ `Some`）。
///
/// ステータスバーのメーターを 100% にする判断に使う。**枠の名前が分かるときだけ**返す
/// のが要点で、`usage credit limit` や支出上限のように 5h / 週へ対応づけられない
/// ものは `None`（= メーターに嘘を書かない。`--` のままにする）。
///
/// 対応は claude 2.1.258 の `nF` 表（`rateLimitType` → 表示名）どおり:
/// `five_hour` = `session limit` / `seven_day` = `weekly limit` /
/// `seven_day_opus` = `Opus limit` / `seven_day_sonnet` = `Sonnet limit`
pub fn exhausted_limit_window(line: &str) -> Option<LimitWindow> {
    if !is_limit_exhausted_line(line) {
        return None;
    }
    let lower = line.to_lowercase();
    if lower.contains("session limit") || lower.contains("5-hour limit") {
        return Some(LimitWindow::FiveHour);
    }
    if lower.contains("weekly limit")
        || lower.contains("opus limit")
        || lower.contains("sonnet limit")
    {
        return Some(LimitWindow::Week);
    }
    None
}

/// `hit your` の直後で限度の名前を探す文字数の歯止め（句読点が来ない行のため）。
/// 実測で最も長い名前は `org's monthly spend limit`（25 文字）
const LIMIT_NAME_MAX_CHARS: usize = 40;

/// `TAKO_1093_LEGACY=1` で #1093 前の文言判定（`hit your usage limit` 決め打ち）へ戻す（A/B 用）
fn legacy_session_limit() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1093_LEGACY").is_some())
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
        find_time_of_day(&lower[pos + anchor.len()..])
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

/// アンカー直後から時刻表記を探す（**日付が挟まっていても読む**。#985）。
///
/// codex 0.150.1 は 2 つの形を出す（バイナリ内の書式文字列で確認）:
/// - 同じ日: `try again at 4:24 AM.`（アンカーの直後が時刻 = 従来の形）
/// - 日をまたぐ: `Try again at Aug 28th, 2026 4:24 AM.`（**日付が挟まる**）
///
/// 直後に読めなければ**短い範囲だけ**前へ進んで探す。進んだ先では
/// 「`:` か am/pm を伴う」ことを必須にする — そうしないと `28th` の 28 や
/// `2026` の一部を時刻と読んでしまう（`Aug 28th, 2026` で実際に踏む形）
fn find_time_of_day(rest: &str) -> Option<i64> {
    // アンカー直後の解釈は従来どおり（**ここだけは裸の時（`resets 3`）も許す**）
    if let Some(t) = parse_time_of_day(rest) {
        return Some(t);
    }
    if legacy_reset_parse() {
        return None;
    }
    let mut prev_digit = false;
    for (i, ch) in rest.char_indices() {
        if i >= RESET_SCAN_BYTES {
            break;
        }
        let digit = ch.is_ascii_digit();
        // 数字の途中からは読まない（`2026` の `026` を時刻にしない）
        if digit && !prev_digit {
            let seg = &rest[i..];
            if is_explicit_time(seg) {
                if let Some(t) = parse_time_of_day(seg) {
                    return Some(t);
                }
            }
        }
        prev_digit = digit;
    }
    None
}

/// 「時刻だと言い切れる形」か = 1〜2 桁の数字のあとに `:` が続く（`4:24`）か、
/// am/pm が続く（`3am` / `3 pm`）。日付の一部（`28th` / `2026`）を弾くための必須条件
fn is_explicit_time(seg: &str) -> bool {
    let digits = seg.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 2 {
        return false; // `2026` のような 3 桁以上は時刻ではない
    }
    let tail = &seg[digits..];
    tail.starts_with(':') || {
        let t = tail.trim_start();
        t.starts_with("am") || t.starts_with("pm")
    }
}

/// アンカーから時刻表記を探す範囲（バイト）。`Aug 28th, 2026 ` を跨げる長さで、
/// 関係ない数字まで届かない程度に短く取る
const RESET_SCAN_BYTES: usize = 40;

/// `TAKO_985_LEGACY=1` で #985 前の解釈（アンカー直後だけ）へ戻す（A/B 用）
fn legacy_reset_parse() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_985_LEGACY").is_some())
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

    // --- #1093: 上限に達した見出し行の判定 ---

    /// 実採取（2026-09-03。univ アカウントの worker 3 体が止まっていた画面）。
    /// **組織のクレジット上限**なので 2 行目が `/usage-credits …` になる
    /// （claude 2.1.258 のバイナリ内文字列と一致）
    const SESSION_LIMIT_HEADLINE: &str =
        "  ⎿  You've hit your session limit · resets 7:50pm (Asia/Tokyo)";

    #[test]
    fn issue1093_組織クレジット上限の見出しを上限として読む() {
        assert!(
            is_limit_exhausted_line(SESSION_LIMIT_HEADLINE),
            "#1093 の実採取見出しが上限として読めていない"
        );
        assert_eq!(
            exhausted_limit_window(SESSION_LIMIT_HEADLINE),
            Some(LimitWindow::FiveHour),
            "`session limit` は 5h 枠（claude の five_hour）"
        );
    }

    #[test]
    fn issue1093_見出しテンプレートの限度名を網羅する() {
        // claude 2.1.258 の `nF` 表 + 支出上限 + 総称。テンプレートは
        // `You've hit your ${名前}${理由}` の 1 本なので、名前が増えても規則で追従する
        for (name, want) in [
            ("session limit", Some(LimitWindow::FiveHour)),
            ("weekly limit", Some(LimitWindow::Week)),
            ("Opus limit", Some(LimitWindow::Week)),
            ("Sonnet limit", Some(LimitWindow::Week)),
            // 枠へ対応づけられないものは**メーターに嘘を書かない**（None）が、
            // 停止としては読む（自動復帰の対象になる）
            ("Fable limit", None),
            ("usage credit limit", None),
            ("individual spend limit", None),
            ("monthly spend limit", None),
            ("org's monthly spend limit", None),
            ("org's monthly usage limit", None),
            ("usage limit", None),
            ("limit", None),
        ] {
            let line = format!("You've hit your {name} · resets 7:50pm (Asia/Tokyo)");
            assert!(
                is_limit_exhausted_line(&line),
                "「{name}」の見出しが上限として読めていない"
            );
            assert_eq!(
                exhausted_limit_window(&line),
                want,
                "「{name}」の枠の読みが想定と違う"
            );
            // 解除時刻は同じアンカー（`resets `）で読める
            let reference = 1_786_752_000 - 9 * 3600 + 30 * 60;
            assert_eq!(
                parse_reset_at(&line, reference, 9 * 3600),
                Some(1_786_752_000 - 9 * 3600 + 19 * 3600 + 50 * 60),
                "「{name}」の見出しから解除時刻が読めていない"
            );
        }
    }

    #[test]
    fn issue1093_解除時刻の無い見出しも上限として読む() {
        // 組織上限は「解除時刻ではなく管理者へ依頼」の案内になることがある
        // （バイナリ内: ` · contact your admin to increase it` / `Tet()`）。
        // 停止としては読み、時刻は不明（core 側の猶予に落ちる）
        for suffix in [
            " · contact your admin to increase it",
            " · ask your admin for a higher limit",
            " · run /usage-credits to ask your admin for a higher limit",
            " · progress saved",
        ] {
            let line = format!("You've hit your session limit{suffix}");
            assert!(is_limit_exhausted_line(&line), "{suffix} で読めていない");
            let reference = 1_786_752_000;
            assert_eq!(
                parse_reset_at(&line, reference, 9 * 3600),
                None,
                "{suffix} に時刻は書かれていない"
            );
        }
    }

    #[test]
    fn issue1093_旧来の文言も同じ関数で読める() {
        // #157 / #748 / #985 の実採取。ここが回帰すると既存の自動復帰が丸ごと死ぬ
        for line in [
            "  ⎿  Claude usage limit reached. Your limit will reset at 3am.",
            "5-hour limit reached ∙ resets 3am",
            "■ You've hit your usage limit. Upgrade to Pro or try again at 4:24 AM.",
        ] {
            assert!(
                is_limit_exhausted_line(line),
                "既存の検知が壊れている: {line}"
            );
        }
    }

    #[test]
    fn issue1093_停止しない告知や警告を上限と読まない() {
        for line in [
            // 自動モデル切替の告知（worker は止まらない。#157 の除外条件）
            "⎿ Claude Opus 4.6 limit reached, now using Claude Sonnet 4.5",
            "5-hour limit reached, now using Claude Sonnet 4.5",
            // 接近の警告（まだ止まっていない。バイナリ内 `You're close to your …`）
            "You're close to your usage limit",
            "You're close to your usage credit limit",
            // 上限を上げる案内（到達していない）
            "/upgrade to increase your usage limit.",
            "Your admin can enable extra usage at claude.ai/admin-settings/usage.",
            // 上限ダイアログの選択肢（「session limit」を含むが到達の見出しではない）
            "     2. Upgrade to Max 20x for higher session limits every month",
            "Continue now at lower priority after reaching your session limit; run again to stop",
            // 地の文での遠い共起（`hit your` の直後に限度の名前が無い）
            "If you hit your head on this design, the rate limit docs explain why",
            // 通常の出力
            "⏺ 実装が完了しました。テストは全て緑です。",
        ] {
            assert!(
                !is_limit_exhausted_line(line),
                "上限ではない行を上限と読んでいる: {line}"
            );
            assert_eq!(exhausted_limit_window(line), None, "{line}");
        }
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

    // --- #985: codex の実文言に対する回帰 ---

    /// codex 0.150.1 のバイナリ内書式（`" Try again at "` + `", %Y %-I:%M %p"`）が作る
    /// **日付つき**の形。#985 前はここが読めず「不明」に落ち、900 秒の猶予で
    /// 早すぎる再開を 3 回撃って諦めていた（= 上限が解けても朝まで止まったまま）
    #[test]
    fn issue985_codexの日付つきリセット時刻を読む() {
        let reference = REF_MIDNIGHT_JST + 30 * 60; // 00:30 JST に観測
        for text in [
            "■ You've hit your usage limit. ... try again at Aug 28th, 2026 4:24 AM.",
            "You've hit your usage limit. Try again at Aug 28, 2026 4:24 AM.",
            "Try again at Sep 3rd, 2026 4:24 AM or try again later.",
        ] {
            assert_eq!(
                parse_reset_at(text, reference, JST),
                Some(REF_MIDNIGHT_JST + 4 * 3600 + 24 * 60),
                "日付を挟んだ時刻が読めない: {text}"
            );
        }
    }

    /// 従来の形（アンカー直後が時刻）は**1 ビットも変えない**
    #[test]
    fn issue985_従来のリセット時刻表記に回帰しない() {
        let reference = REF_MIDNIGHT_JST + 30 * 60;
        let cases = [
            (
                "Claude usage limit reached. Your limit will reset at 3am.",
                3 * 3600,
            ),
            ("5-hour limit reached ∙ resets 3am", 3 * 3600),
            (
                "You've hit your usage limit. Please try again at 4:24 AM.",
                4 * 3600 + 24 * 60,
            ),
            ("Your limit will reset at 15:30 (JST)", 15 * 3600 + 30 * 60),
        ];
        for (text, tod) in cases {
            assert_eq!(
                parse_reset_at(text, reference, JST),
                Some(REF_MIDNIGHT_JST + tod),
                "{text}"
            );
        }
    }

    /// 日付の数字を時刻と読み違えない（`28th` の 28 / `2026` の一部）
    #[test]
    fn issue985_日付の数字を時刻と読み違えない() {
        let reference = REF_MIDNIGHT_JST + 30 * 60;
        // 時刻が無ければ「不明」に落ちる（誤った時刻を作らない）
        assert_eq!(
            parse_reset_at("Try again at Aug 28th, 2026.", reference, JST),
            None,
            "日付だけの行から時刻を捏造してはいけない"
        );
        assert_eq!(
            parse_reset_at("Try again at some point next week.", reference, JST),
            None
        );
        // 遠くにある無関係な数字は拾わない（走査範囲を超える）
        let far = format!("Try again at {} 4:24 AM.", "x".repeat(60));
        assert_eq!(parse_reset_at(&far, reference, JST), None);
    }

    /// #985 受け入れ条件 4: 課金・モデル変更・**限りある資源の引き換え**を選ばない。
    /// ラベルは codex 0.150.1 のバイナリ内文字列と claude の実採取から採った
    #[test]
    fn issue985_課金や資源消費の選択肢は決して選ばない() {
        let unsafe_labels = [
            // claude（実採取）
            "Upgrade to Max 20x for higher session limits every month",
            "Continue with usage credits",
            // codex（バイナリ内文字列）
            "Switch to gpt-5.4-mini",
            "Upgrade to Plus to continue using Codex",
            "Visit https://chatgpt.com/codex/settings/usage to purchase more credits",
            "Request a limit increase from your owner to continue using codex",
            "Yes, use reset",
            "Get More AI Credits",
            "Ask your workspace owner to add more",
        ];
        for label in unsafe_labels {
            assert!(
                !is_safe_choice_label(label),
                "自動確定してはいけないラベルが許可された: {label}"
            );
            // 単独で並んでいても選ばれない（= 何もしないほうへ落ちる）
            assert_eq!(
                safe_choice(&[(Some(1), label.to_string())]),
                None,
                "{label}"
            );
        }
    }

    /// codex の「Use this reset?」ダイアログでは**何も選ばない**（在庫のあるリセットを
    /// 勝手に引き換えない）。「No, go back」は安全に見えるが、選ぶ理由が無いので選ばない
    #[test]
    fn issue985_codexのリセット引き換えダイアログでは何も選ばない() {
        let opts = opts(&["Yes, use reset", "No, go back", "Choose a different reset."]);
        assert_eq!(safe_choice(&opts), None);
    }

    /// codex の「Approaching rate limits」だけは安全な出口がある（現状維持）
    #[test]
    fn issue985_codexの接近ダイアログは現状維持を選ぶ() {
        let opts = opts(&[
            "Switch to gpt-5.4-mini",
            "Keep current model",
            "Keep current model (never show again)",
        ]);
        assert_eq!(safe_choice(&opts), Some((2, "Keep current model")));
    }
}

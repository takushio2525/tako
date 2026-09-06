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

/// **日付を持たない**時刻表記から解決したリセット時刻として受け入れる最大の待ち時間。
/// これを超える値は誤パースとみなして「不明」に落とす（時刻表記は 24 時間で 1 周する）。
/// 日付つきの表記は別の上限（[`MAX_DATED_WAIT_SECS`]）で受ける（#1096）
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
/// # テンプレートに載らない「尽きた」告知（#1096）
///
/// claude は同じ「もう進めない」状態を**別の書き出し**でも出す。同バイナリはその前置きを
/// 自分で列挙している（`dCt`。並びは阻害 → 警告 → 情報の 3 つに分かれていて、
/// `fCt = ["You've used", "You're close to"]` は**警告**・
/// `mCt = ["You're now using usage credits", …]` は**情報**なので受けない）:
///
/// ```text
/// dCt = ["You've hit your", "You've reached your", "You're out of usage credits",
///        "Your org is out of usage · add funds to continue",
///        "Your org is out of usage · contact your admin",
///        "Your seat type doesn't include usage credits", …]
/// ```
///
/// このうち**時間で解ける枠**だけを受ける = 動詞 2 種（`hit your` / `reached your`）と
/// `out of usage credits` / `org is out of usage`。座席種別・組織の無効化・
/// `group's usage limit is set to $0` のように**時間では解けない**ものは入れない
/// —— 受けると `WorkerErrorKind::UsageLimit` の助言（`wait_reset` = 解除まで待つ）が
/// 嘘になる。そちらは [`entitlement_block_line`] が受け持ち、**この関数は
/// そちらに当たる行を必ず false にする**（#1107。排他を構造で保証する）。
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
    // **時間では解けない阻害はここでは受けない**（#1107）。2 つの規則を排他にしておくと
    // 種別が検査順に依らず一意に決まり、メーター（`exhausted_limit_window`）も
    // 自動的に `--` のままになる。codex の `You've reached your workspace credit limit`
    // は見出しだけ見るとテンプレート（`reached your <名前>limit`）にも当たるので、
    // **偶然の排他ではなく構造で**分けておく必要がある
    if entitlement_block_line(line) {
        return false;
    }
    if legacy_session_limit() {
        // #1093 前の規則（`hit your usage limit` 決め打ち）へ戻す A/B 用
        return line.contains("hit your usage limit")
            || line.contains("usage limit reached")
            || (line.contains("limit reached") && line.contains("reset"));
    }
    // テンプレートに載らない書き出し（#1096）。前置きそのものが「尽きた」を意味するので
    // 限度の名前を探す必要が無い（`Your org is out of usage · contact your admin` には
    // `limit` という語すら無い）
    if !legacy_out_of_usage() && EXHAUSTED_PHRASES.iter().any(|p| line.contains(p)) {
        return true;
    }
    // テンプレート `You've hit your <限度の名前><理由や解除時刻>`（動詞は #1096 で 2 種）。
    // 限度の名前は動詞の**直後**にあり、後ろに続く理由や解除時刻は必ず
    // `·` か `.` で始まる（`… session limit · resets 7:50pm` /
    // `… usage limit. Upgrade to Pro …`）。そこで**名前の区間だけ**を見る。
    // `,` は限度の名前には現れないので、地の文（「… hit your head on this design,
    // the rate limit docs …」）を上限と読まないための追加の境界として足してある。
    // 句読点が来ない病的な行には文字数で歯止めをかける
    for verb in EXHAUSTED_HEADLINE_VERBS {
        if legacy_out_of_usage() && verb != "hit your" {
            continue; // #1096 前は `hit your` だけを見ていた
        }
        let Some((_, rest)) = line.split_once(verb) else {
            continue;
        };
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

/// 「時間では解けない」利用阻害を告げる行か（Issue #1106）。
///
/// **[`is_limit_exhausted_line`] とは意図的に別の関数**にしてある。混ぜると自動復帰
/// （#813）が「解除まで待つ」で撃ち始めてしまうが、ここで受けるものは待っても解けない
/// —— 座席種別・管理者による無効化・グループ枠 $0・クレジットの要求・追加利用ぶんの
/// 枯渇・組織でのサービス無効はいずれも**人（管理者 / プラン / クレジット）の対処**が要る。
/// 呼び出し側（`tako-control::orchestrator::wait::detect_worker_error`）は
/// `WorkerErrorKind::EntitlementBlocked`（推奨アクション `needs_human`）として返す。
///
/// # 根拠（claude 2.1.258 のバイナリ実測。#1096 と同じ表）
///
/// claude は前置きを自分で 3 分類している。受けるのは**阻害**（`dCt` / `pCt` / `Par`）の
/// うち、#1096 が時間で解けるものとして取った 4 条件（`hit your` / `reached your` /
/// `out of usage credits` / `org is out of usage`）を除いた残り:
///
/// ```text
/// dCt = [… "Your seat type doesn't include usage credits",
///           "Your seat type doesn't include usage",
///           "Your seat type doesn't include extra usage",
///           "Your usage allocation has been disabled by your admin",
///           "Your group's usage limit is set to $0",
///           "Fable 5 requires usage credits",
///           "You're out of extra usage"]
/// Par = [/^Fable(?: [^·\n]{1,40})? requires usage credits\./]  // Fable の総称形
/// pCt = ["This service is disabled for your org"]
/// ```
///
/// = 6 分類 / 8 文言。`fCt`（`You've used` / `You're close to` = 警告）と
/// `mCt`（`You're now using usage credits` = 情報）は**受けない**（#1096 で確定した
/// とおり、まだ動けるペインを止まったと読まないため）。
///
/// # なぜ部分一致の組で判定するのか
///
/// アポストロフィは**判定に使わない**（#1093 / #1096 と同じ方針）。同じバイナリの中で
/// ASCII の `'` と U+2019 が混在しており、どちらかに寄せると片方を取りこぼす。
/// そこで `doesn't` / `group's` / `You're` のようにアポストロフィを含む語は
/// **その手前と後ろに分けた必須語の組**（AND）で表す。
///
/// 版が上がって文言が増えたら `dCt` / `pCt` / `Par` を読み直すのが正しい採り方
/// （#748 / #985 / #1093 / #1096 と同じ作法）。
pub fn entitlement_block_line(line: &str) -> bool {
    if legacy_entitlement_block() {
        return false; // #1106 前（8 文言をどこでも受けなかった）へ戻す A/B 用
    }
    let lower = line.to_lowercase();
    ENTITLEMENT_BLOCK_PHRASES
        .iter()
        .any(|needles| needles.iter().all(|n| lower.contains(n)))
}

/// 上限の見出しが使う動詞（claude 2.1.258 の `dCt` の先頭 2 件。#1096）。
/// `You've reached your Fable limit.` は `hit` ではないので #1093 の規則では
/// 当たらなかった（#1093 とまったく同じ機序の取りこぼし）
const EXHAUSTED_HEADLINE_VERBS: [&str; 2] = ["hit your", "reached your"];

/// テンプレートに載らない「尽きた」告知のうち**時間で解ける**ぶん（#1096）。
/// アポストロフィ（`You're` / `Your org`）は判定に使わない
const EXHAUSTED_PHRASES: [&str; 2] = ["out of usage credits", "org is out of usage"];

/// 時間では解けない利用阻害の語句（#1106 / #1107）。各要素は**すべて含まれていること**を
/// 求める必須語の組（アポストロフィを避けて分割してある。[`entitlement_block_line`]）。
/// 並びは A〜F = claude（Issue #1106 の分類表）/ G〜J = codex・agy（#1107 の実物調査）
const ENTITLEMENT_BLOCK_PHRASES: [&[&str]; 11] = [
    // A. 座席種別（`Your seat type doesn't include usage credits` /
    //    `… doesn't include usage` / `… doesn't include extra usage`）。
    //    アポストロフィの前後で割る（`doesn't` / `doesn\u{2019}t` の両方に効き、
    //    「The seat type field does not include usage …」のような地の文には当たらない）。
    //    `extra usage` は `include usage` を含まないので別の組にする
    &["seat type doesn", "t include usage"],
    &["seat type doesn", "t include extra usage"],
    // B. 管理者による無効化（`Your usage allocation has been disabled by your admin`）
    &["usage allocation has been disabled"],
    // C. グループ枠 $0（`Your group's usage limit is set to $0`）。
    //    `limit` という語はあるが動詞が無いので #1096 の規則では当たらない
    &["usage limit is set to $0"],
    // D. クレジットの要求（`Fable 5 requires usage credits` + `Par` の総称形。
    //    モデル名を判定に入れないので版でモデルが増えても追従する）
    &["requires usage credits"],
    // E. 追加利用ぶんの枯渇（`You're out of extra usage`）。
    //    #1096 の `out of usage credits` とは別の文言
    &["out of extra usage"],
    // F. 組織でサービス無効（`pCt` = `This service is disabled for your org`）
    &["service is disabled for your org"],
    // --- ここから codex / agy（#1107 の実物調査）---
    // G. クレジット残高の枯渇。**owner / 本人が買い足すまで解けない**
    //    codex 0.153.0: `You're out of credits.` /
    //      `Your workspace is out of credits. Add credits to continue.` /
    //      `… Ask your workspace owner to refill in order to continue.` /
    //      `… Add credits to continue using Codex.`
    //    agy 1.1.25: `AI: Out of credits`（TUI のエラー表示。前払いクレジットが 0）
    //    claude のバイナリには 1 件も無い（`out of usage credits` = 時間で解ける別文言）
    &["out of credits"],
    // H. spend cap（codex 0.153.0）。**上限を上げるまで解けない**
    //    `You hit your spend cap set in your workspace. Increase your spend cap to continue.` /
    //    `You hit your spend cap set by the owner of your workspace. Ask an owner to …`
    //    claude の `spend limit`（75 件）とは別語で、こちらは claude に 1 件も無い
    &["spend cap"],
    // I. workspace のクレジット上限（codex 0.153.0 のダイアログ見出し）。
    //    `You've reached your workspace credit limit` +
    //    `Your workspace is out of credits. Ask your workspace owner to add more. Notify owner?`
    //    **見出しだけは `reached your … limit` のテンプレートにも当たる**が、
    //    対処の選択肢に「待つ」出口が無い（`Notify owner?` だけ）ので時間では解けない
    &["workspace credit limit"],
    // J. ライセンス不足（agy 1.1.25）。**管理者が付与するまで解けない**
    //    `No license available for this project and location. Contact your administrator
    //     to setup Gemini Enterprise for this project.`
    &["no license available"],
];

/// 動詞の直後で限度の名前を探す文字数の歯止め（句読点が来ない行のため）。
/// 実測で最も長い名前は `org's monthly spend limit`（25 文字）
const LIMIT_NAME_MAX_CHARS: usize = 40;

/// `TAKO_1093_LEGACY=1` で #1093 前の文言判定（`hit your usage limit` 決め打ち）へ戻す（A/B 用）
fn legacy_session_limit() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1093_LEGACY").is_some())
}

/// `TAKO_1096_LEGACY=1` で #1096 前へ戻す（A/B 用）。
/// 動詞は `hit your` だけ・`You're out of …` 系は受けない・解除時刻の日付も読まない
fn legacy_out_of_usage() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1096_LEGACY").is_some())
}

/// `TAKO_1106_LEGACY=1` で #1106 前へ戻す（A/B 用）。
/// 時間では解けない阻害の 8 文言をどこでも受けない = worker が idle（作業完了）に見える
fn legacy_entitlement_block() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1106_LEGACY").is_some())
}

// --- 折り返しの結合（#1123） ---

/// 結合する継続行の上限。
///
/// 21 桁のペインでは見出し 1 本が 4 行、`⎿` の塊まるごとなら 9 行ほどに割れる（実採取）。
/// 上限を置くのは「字下げが続くだけの領域」を丸ごと 1 本へ畳んでしまわないため
const MAX_WRAP_JOIN_LINES: usize = 12;

/// 結合後の論理行の文字数上限。見出し + 解除時刻は 21 桁の折り返しでも 120 文字前後
/// なので十分に余裕がある。長い出力を延々とつなげないための歯止め
const MAX_WRAP_JOIN_CHARS: usize = 1000;

/// **新しい塊**の始まりを表す先頭文字（claude TUI の実採取より）。
/// これで始まる字下げ行は折り返しの続きとして扱わない。
///
/// - `⎿` = ツール結果 / `⏺` = 発話・ツール実行の見出し / `❯` = 入力行と選択カーソル
/// - `│ ╭ ╮ ╰ ╯ ─ ━ ▔ ▁ █` = 入力欄と区切り線の罫線
///
/// **`·`（中黒）は入れない**: 実採取の `⎿` の塊に `     · clau.de/web` が
/// 継続行として現れる（中黒で始まる継続行は珍しくない）
const BLOCK_MARKERS: [char; 13] = [
    '⎿', '⏺', '❯', '│', '╭', '╮', '╰', '╯', '─', '━', '▔', '▁', '█',
];

/// その行が**新しい論理行**を始めるか（= 直前の行の折り返しの続きではないか）。
///
/// 字下げが無い行は必ず新しい塊（claude は折り返した続きを必ず字下げする = 実測）。
/// 字下げがあっても目印で始まる行は新しい塊
fn starts_new_block(line: &str) -> bool {
    if !line.starts_with(' ') {
        return true; // 字下げ無し（空行もここ）
    }
    let rest = line.trim_start();
    match rest.chars().next() {
        None => true,
        Some(first) => BLOCK_MARKERS.contains(&first),
    }
}

/// 折り返しを結合した論理行を作る（#1123）。空行は塊の切れ目として落とす。
///
/// # なぜ必要か（実測 2026-09-04。claude 2.1.258 を 25 桁のペインで動かした）
///
/// claude TUI は本文を**自分で**折り返し、続きを字下げして次の行へ書く。端末の
/// ソフトラップ（1 論理行が画面幅で折り返されているだけ）ではないので、
/// **`tmux capture-pane -J` でも alacritty の `WRAPLINE` でも 1 本には戻らない**
/// （`-J` で採っても行が割れたままなのを実測した）。
///
/// ```text
///   ⎿  Tip: Run tasks in      ← 目印つきの先頭行（内容は 5 桁目から）
///      the cloud while you    ← 続き（5 桁の字下げ）
///      keep coding locally
///      · clau.de/web
/// ```
///
/// 続きの字下げ幅は**内容の列とは限らない**（同じ画面で `  ⎿  Stop hook` の続きが
/// 2 桁の字下げで出るのを実測した）ので、幅の一致では判定しない。
/// 「字下げがあり、目印で始まらない非空行」を続きとみなす。
///
/// # 何に使うか
///
/// 上限の見出し（[`is_limit_exhausted_line`]）・利用阻害（[`entitlement_block_line`]）・
/// 解除時刻（[`parse_reset_at`]）はどれも **1 論理行**を入力に取る。狭いペイン
/// （実発生は 21〜25 桁）では見出しが 3〜4 行に割れ、どの物理行も規則に当たらないので
/// 自動復帰（#813）も watch の `WORKER_ERROR` も同時に外れる
/// （#1123 の実害 = 解除後 7.5 時間 worker 4 体が止まったまま）。
///
/// **呼び出し側は物理行を先に走査し、外れたときだけここへ落ちる**こと。
/// 結合は候補を増やすだけなので、折り返しの無い画面では判定が 1 ビットも変わらない
pub fn unwrap_wrapped_lines<S: AsRef<str>>(lines: &[S]) -> Vec<String> {
    let legacy = legacy_wrapped_headline();
    let mut out: Vec<String> = Vec::new();
    // 直前の論理行が続きを受け付けるか（空行を挟んだら閉じる）
    let mut open = false;
    let mut joined = 0usize;
    for raw in lines {
        let line = raw.as_ref();
        if line.trim().is_empty() {
            open = false;
            joined = 0;
            continue;
        }
        if !legacy && open && joined < MAX_WRAP_JOIN_LINES && !starts_new_block(line) {
            let tail = line.trim();
            if let Some(last) = out.last_mut() {
                if last.chars().count() + tail.chars().count() < MAX_WRAP_JOIN_CHARS {
                    // claude は語の境界で折り返して行頭の空白を捨てるので、
                    // 空白 1 つでつなぐと元の 1 行が戻る
                    last.push(' ');
                    last.push_str(tail);
                    joined += 1;
                    continue;
                }
            }
        }
        out.push(line.trim_end().to_string());
        open = true;
        joined = 0;
    }
    out
}

/// 画面末尾の論理行を**下から順に**最大 `take` 本返す（#1123）。
///
/// 物理行の走査で外れたときのフォールバックに使う（下の行ほど新しいので末尾から見る）。
/// `TAKO_1123_LEGACY=1` では結合が起きないので、返るのは呼び出し側が既に見た物理行
/// そのもの = 候補が増えず #1123 前の判定へそのまま戻る
pub fn unwrapped_tail<S: AsRef<str>>(lines: &[S], take: usize) -> Vec<String> {
    let mut out = unwrap_wrapped_lines(lines);
    out.reverse();
    out.truncate(take);
    out
}

/// 画面テキスト（改行区切り）から論理行を下から順に返す（#1123）。
/// [`unwrapped_tail`] の `&str` 版で、`detect_worker_error` のように
/// 1 本の文字列を受け取る呼び出し側が使う
pub fn unwrapped_tail_of(output: &str, take: usize) -> Vec<String> {
    let lines: Vec<&str> = output.lines().collect();
    unwrapped_tail(&lines, take)
}

/// `TAKO_1123_LEGACY=1` で #1123 前（折り返しを結合しない）へ戻す（A/B 用）。
/// 狭いペインでは上限の見出しがどの規則にも当たらなくなる
fn legacy_wrapped_headline() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1123_LEGACY").is_some())
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
    // アンカーは前から順に試す。**「日付は在るが読めない」を見つけた時点で打ち切る**のが要点:
    // `resets at ` は `resets ` を含むので、素通りさせると同じ 1 箇所を別のアンカーで
    // 読み直してしまい（`resets at Feb 31, 3pm` → `at feb 31, 3pm`）、
    // 前方走査が `3pm` を拾って「不明へ落とす」判断が骨抜きになる
    for anchor in RESET_ANCHORS {
        let Some(pos) = lower.find(anchor) else {
            continue;
        };
        let rest = &lower[pos + anchor.len()..];
        // 日付が書かれていればそちらが正（#1096）。24 時間より先の解除は必ず日付つきで
        // 出るので、時刻だけを読むと「明日の同じ時刻」へ丸まって早撃ちになる
        if !legacy_out_of_usage() {
            match parse_dated_reset(rest, reference, tz_offset) {
                DatedReset::Parsed(at) => return Some(at),
                // **日付が書いてあるのに読めなかったときは時刻だけの解釈へ落とさない**。
                // 日付が前置きされている = 解除は 24 時間より先なので、時刻だけを読んだ
                // 値は「次に来る同じ時刻」= 高々 24 時間以内で**確実に間違っている**。
                // 自信のある間違った時刻を返すより「不明」を返すほうが、状態照会
                // （`reset_at: null`）と監査ログから書式の変化に気づける。
                // なお「不明」の猶予（`UNKNOWN_RESET_FALLBACK_SECS`）は 15 分なので
                // **初回の試行は早まる**（遅らせるには「読めない」を表す状態が要り、
                // それは `LimitStop` の形を変えるので別の話）
                DatedReset::Unreadable => return None,
                DatedReset::NotDated => {}
            }
        }
        if let Some(tod) = find_time_of_day(rest) {
            if let Some(at) = resolve_time_of_day(tod, reference, tz_offset) {
                return Some(at);
            }
        }
    }
    None
}

/// 日付を持たない時刻表記を「観測時刻から見て次に来る同じ時刻」として解決する
fn resolve_time_of_day(tod: i64, reference: i64, tz_offset: i32) -> Option<i64> {
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

/// 日付つきの解除時刻を読める最大の先（#1096）。週枠は 7 日先まで、codex の
/// クレジットは月単位まで出るので余裕を持たせる。日付なしの表記（[`MAX_PARSED_WAIT_SECS`]）
/// とは別の上限にしてある —— あちらは「次に来る同じ時刻」への丸めが安全である範囲
const MAX_DATED_WAIT_SECS: i64 = 60 * 86_400;

/// 日付つきの解除時刻が過去でも受け入れる幅（#1096）。
/// 「もう解けている」を正しく読むために少しだけ過去を許す
const DATED_PAST_TOLERANCE_SECS: i64 = 86_400;

/// 月名の先頭 3 文字（`toLocaleString("en-US", {month:"short"})` の出力。小文字で比較）
const MONTH_PREFIXES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// アンカー直後の**日付つき**解除時刻を絶対時刻（unix 秒）として読む（#1096）。
///
/// claude の時刻整形（`Rd`）は解除が 24 時間より先だと日付を前置きする（実測 = バイナリ内の
/// `{month:"short", day:"numeric", hour:"numeric", minute:…, hour12:true}` + 年が違えば
/// `year:"numeric"`）。**週枠（`weekly limit` / `Opus limit`）は最大 7 日先なので
/// 日付つきが通常形**で、時刻だけ読むと「明日」へ丸まって解除の数日前から撃ってしまう。
///
/// 読める形（すべて実測）:
///
/// | 出どころ | 形 |
/// |---|---|
/// | claude（24h 超） | `Sep 8, 3pm (Asia/Tokyo)` / `Sep 8, 3:05pm (Asia/Tokyo)` |
/// | claude（年が違う） | `Sep 8, 2027, 3:05pm (Asia/Tokyo)` |
/// | codex | `Aug 28th, 2026 4:24 AM`（序数 + 年・年と時刻のあいだにカンマ無し） |
///
/// 年が書かれていないときは**観測時刻から見て最も近い年**を採る（12 月 → 1 月の
/// 年またぎをここで吸収する）。範囲外（60 日より先 / 1 日より前）は誤パースとみなす
fn parse_dated_reset(rest: &str, reference: i64, tz_offset: i32) -> DatedReset {
    let b = rest.as_bytes();
    let mut i = skip_spaces(b, 0);
    // 月名（先頭 3 文字で判定し、`sept` のような綴りは英字が続くぶんだけ読み飛ばす）
    let Some(month) = MONTH_PREFIXES
        .iter()
        .position(|m| rest[i..].starts_with(m))
        .map(|p| p + 1)
    else {
        return DatedReset::NotDated;
    };
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    i = skip_spaces(b, i);
    // 日
    let Some((day, next)) = read_number(b, i, 2) else {
        return DatedReset::Unreadable;
    };
    i = next;
    // 序数の接尾辞（`28th`）
    for suffix in ["st", "nd", "rd", "th"] {
        if rest[i..].starts_with(suffix) {
            i += suffix.len();
            break;
        }
    }
    i = skip_separators(b, i);
    // 年（4 桁。無ければ観測時刻から推定する）
    let mut year = None;
    if let Some((y, next)) = read_number(b, i, 4) {
        if next - i == 4 {
            year = Some(y);
            i = skip_separators(b, next);
        }
    }
    let Some(tod) = parse_time_of_day(&rest[i..]) else {
        return DatedReset::Unreadable;
    };
    let local_ref = reference + i64::from(tz_offset);
    let (ref_year, _, _) = civil_from_days(local_ref.div_euclid(86_400));
    // 年が書かれていればそれだけ、無ければ前後 1 年を候補にして最も近いものを採る
    let candidates: Vec<i64> = match year {
        Some(y) => vec![y],
        None => vec![ref_year, ref_year + 1, ref_year - 1],
    };
    let mut best: Option<i64> = None;
    for y in candidates {
        let days = days_from_civil(y, month as i64, day);
        // `Feb 31` のような実在しない日付を弾く（往復して一致するかで見る）
        if civil_from_days(days) != (y, month as i64, day) {
            continue;
        }
        let target_local = days * 86_400 + tod;
        let wait = target_local - local_ref;
        if !(-DATED_PAST_TOLERANCE_SECS..=MAX_DATED_WAIT_SECS).contains(&wait) {
            continue;
        }
        // 未来を優先し、同じなら近いほうを採る
        let better = best.is_none_or(|b| {
            let cur = b - local_ref;
            (wait >= 0, -wait.abs()) > (cur >= 0, -cur.abs())
        });
        if better {
            best = Some(target_local);
        }
    }
    match best {
        Some(target_local) => DatedReset::Parsed(target_local - i64::from(tz_offset)),
        None => DatedReset::Unreadable,
    }
}

/// 月名（`toLocaleString("en-US", {month:"short"})` の出力そのまま）
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// **日付つき**解除時刻の表記を組む（[`parse_dated_reset`] の逆。#1096）。
///
/// 検証用の fixture のために置いてある。日付を fixture へ焼き込むとその日が過ぎた瞬間に
/// 壊れる（#985 のセルフテストは `Aug 28th, 2026` を焼き込んでいたので 2026-09-03 には
/// **6 日前**を指しており、日付を読むようにした途端「範囲外」で解決できなくなった）ので、
/// 実行時に未来の日付から作れるようにする。逆関数を同じモジュールに置くことで、
/// 書式の理解が fixture とパーサでずれない
///
/// 出力は codex の実測形（`Aug 28th, 2026 4:24 AM`）。claude の形（`Sep 8, 3:05pm`）とは
/// 序数・年・AM/PM の大小が違うが、どちらも [`parse_dated_reset`] が読む。
///
/// **精度は分まで**（書式が `h:mm` なので秒は落ちる）。読み直した値と比べるときは
/// 期待値も分へ丸めること
pub fn format_dated_reset(at: i64, tz_offset: i32) -> String {
    let local = at + i64::from(tz_offset);
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let tod = local.rem_euclid(86_400);
    let (h24, minute) = (tod / 3_600, (tod % 3_600) / 60);
    let (h12, ampm) = match h24 {
        0 => (12, "AM"),
        1..=11 => (h24, "AM"),
        12 => (12, "PM"),
        _ => (h24 - 12, "PM"),
    };
    let ord = match (d % 10, d % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    let mon = MONTH_NAMES[(m - 1).clamp(0, 11) as usize];
    format!("{mon} {d}{ord}, {y} {h12}:{minute:02} {ampm}")
}

/// アンカー直後に日付が書かれていたかと、読めたか（#1096）
enum DatedReset {
    /// 日付は書かれていない（時刻だけの従来解釈へ進む）
    NotDated,
    /// 日付つきで読めた（絶対時刻）
    Parsed(i64),
    /// 日付は書かれているが読めない / 範囲外。**時刻だけの解釈へ落とさない**
    Unreadable,
}

fn skip_spaces(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] == b' ' {
        i += 1;
    }
    i
}

/// 空白とカンマを読み飛ばす（`Sep 8, 2027, 3:05pm` の区切り）
fn skip_separators(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i] == b' ' || b[i] == b',') {
        i += 1;
    }
    i
}

/// 最大 `max_digits` 桁の 10 進数を読む（読めた値と次の位置）
fn read_number(b: &[u8], start: usize, max_digits: usize) -> Option<(i64, usize)> {
    let mut i = start;
    let mut v: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() && i - start < max_digits {
        v = v * 10 + i64::from(b[i] - b'0');
        i += 1;
    }
    if i == start {
        return None;
    }
    Some((v, i))
}

/// グレゴリオ暦 → 1970-01-01 からの日数（Howard Hinnant の `days_from_civil`）。
/// 外部クレートを増やさない自前変換（`pane_log::civil_utc` の逆変換）
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 1970-01-01 からの日数 → グレゴリオ暦（`days_from_civil` の逆）
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
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

    // --- #1096: `You're out of …` テンプレートと日付つき解除時刻 ---

    #[test]
    fn issue1096_outofusage系の別テンプレートを上限として読む() {
        // claude 2.1.258 の `dCt`（阻害の前置き）から、**時間で解ける**ぶんの実文言。
        // `Your org is out of usage · contact your admin` には `limit` という語すら無く、
        // #1093 の規則（`hit your` + `limit`）では原理的に当たらない
        for line in [
            "You're out of usage credits · resets 7:50pm (Asia/Tokyo)",
            "You're out of usage credits · resets 7:50pm (Asia/Tokyo) · progress saved",
            "You're out of usage credits. Switch to another model to continue.",
            "You're out of usage credits. Run /usage-credits to keep using Opus or /model to switch models.",
            "You're out of usage credits. /model to switch models.",
            "Your org is out of usage · add funds to continue",
            "Your org is out of usage · contact your admin",
            // 動詞が `reached`（`dCt` の 2 番目）。#1093 は `hit` 決め打ちだった
            "You've reached your Fable limit. Switch to another model to continue.",
            "You've reached your weekly limit · resets 7:50pm (Asia/Tokyo)",
        ] {
            assert!(
                is_limit_exhausted_line(line),
                "上限として読めていない: {line}"
            );
        }
    }

    #[test]
    fn issue1096_クレジット系は枠へ対応づけずメーターに嘘を書かない() {
        // `usage credits` は 5h / 週のどちらでもないので `--` のまま
        for line in [
            "You're out of usage credits · resets 7:50pm (Asia/Tokyo)",
            "Your org is out of usage · contact your admin",
            "You've reached your Fable limit. Switch to another model to continue.",
        ] {
            assert!(is_limit_exhausted_line(line));
            assert_eq!(
                exhausted_limit_window(line),
                None,
                "枠が分からないのに対応づけている: {line}"
            );
        }
        // 枠の名前が書いてあるものは従来どおり読む（動詞が `reached` でも）
        assert_eq!(
            exhausted_limit_window("You've reached your weekly limit · resets 7:50pm"),
            Some(LimitWindow::Week)
        );
    }

    #[test]
    fn issue1096_警告と情報の前置きは上限と読まない() {
        // claude 自身が別リストに分けているもの:
        // `fCt = ["You've used", "You're close to"]`（警告）
        // `mCt = ["You're now using usage credits", …]`（情報）
        for line in [
            "You've used 75% of your weekly limit",
            "You're close to your usage credit limit",
            "You're close to your weekly limit · resets 7:50pm",
            "You're now using usage credits · Your weekly limit resets 7:50pm",
            "You're now using your usage allocation",
            "Now using usage credits for Opus",
            "Now using your usage allocation",
        ] {
            assert!(
                !is_limit_exhausted_line(line),
                "警告 / 情報を上限（停止）と読んでいる: {line}"
            );
        }
    }

    // --- #1106: 時間では解けない利用阻害（`UsageLimit` とは別種） ---

    /// claude 2.1.258 の `dCt` / `pCt` / `Par` から採った実文言（6 分類 / 8 文言）。
    /// アポストロフィは ASCII の `'` と U+2019 の両方を並べる（判定に使っていないことの固定）
    const ENTITLEMENT_LINES: [&str; 11] = [
        // A. 座席種別（3 文言）
        "Your seat type doesn't include usage credits",
        "Your seat type doesn't include usage",
        "Your seat type doesn't include extra usage",
        "Your seat type doesn\u{2019}t include usage credits",
        // B. 管理者による無効化
        "Your usage allocation has been disabled by your admin",
        // C. グループ枠 $0
        "Your group's usage limit is set to $0",
        "Your group\u{2019}s usage limit is set to $0",
        // D. クレジットの要求（実文言 + `Par` の総称形）
        "Fable 5 requires usage credits",
        "Fable 5.1 Sonnet requires usage credits. Run /usage-credits to continue.",
        // E. 追加利用ぶんの枯渇
        "You're out of extra usage",
        // F. 組織でサービス無効（`pCt`）
        "This service is disabled for your org",
    ];

    /// codex 0.153.0 / agy 1.1.25 のバイナリから採った実文言（#1107 の実物調査）。
    /// **claude のバイナリには 1 件も無い**（`out of credits` 0 件 / `spend cap` 0 件 /
    /// `workspace credit limit` 0 件 / `no license available` 0 件 = 実測）ので、
    /// claude の判定は 1 ビットも変わらない
    const ENTITLEMENT_LINES_CODEX_AGY: [&str; 8] = [
        // G. クレジット残高の枯渇（codex）
        "You're out of credits.",
        "Your workspace is out of credits. Add credits to continue.",
        "Your workspace is out of credits. Ask your workspace owner to refill in order to continue.",
        "Your workspace is out of credits. Add credits to continue using Codex.",
        // G. 同（agy 1.1.25 の TUI エラー表示。前払いクレジットが 0）
        "AI: Out of credits",
        // H. spend cap（codex）
        "You hit your spend cap set in your workspace. Increase your spend cap to continue.",
        "You hit your spend cap set by the owner of your workspace. Ask an owner to increase your spend cap to continue.",
        // J. ライセンス不足（agy）
        "No license available for this project and location. Contact your administrator to setup Gemini Enterprise for this project.",
    ];

    #[test]
    fn issue1107_codexとagyの阻害文言も別種として読む() {
        for line in ENTITLEMENT_LINES_CODEX_AGY {
            assert!(
                entitlement_block_line(line),
                "時間で解けない阻害として読めていない: {line}"
            );
            // 上限停止（`wait_reset`）とは排他 = 自動復帰が撃たない
            assert!(
                !is_limit_exhausted_line(line),
                "時間で解けない阻害を上限停止と読んでいる: {line}"
            );
            assert_eq!(exhausted_limit_window(line), None, "{line}");
        }
    }

    #[test]
    fn issue1107_workspace_credit_limitはテンプレートより阻害が優先する() {
        // codex 0.153.0 のダイアログ見出し。**`reached your <名前>limit` の
        // テンプレートにも当たる**ので、排他を構造で保証していないと
        // `usage_limit`（= 解除まで待つ）に倒れて助言が嘘になる。
        // 対処の選択肢は `Ask your workspace owner to add more. Notify owner?` だけで
        // 「待つ」出口が無い（= 時間では解けない）
        let line = "You've reached your workspace credit limit";
        assert!(entitlement_block_line(line));
        assert!(
            !is_limit_exhausted_line(line),
            "テンプレート側が勝っている（排他が構造で保証されていない）"
        );
        // 語句を外した形（= 素のテンプレート）は従来どおり上限として読む。
        // ここは **`TAKO_1093_LEGACY` / `TAKO_1096_LEGACY` のどのアームでも成立する形**
        // を選ぶ（legacy アームの失敗集合を #1107 で増やさないため）
        assert!(is_limit_exhausted_line("You've hit your usage limit"));
    }

    #[test]
    fn issue1107_codexの上限系は従来どおり時間で解ける扱い() {
        // codex 0.153.0 の実文言。こちらは「上限」なので待てば解ける
        // （#985 の分類を変えない。`Request a limit increase from your owner` は
        // 境界事例だが、文言が `Usage limit reached` なので上限側に残す）。
        //
        // **`is_limit_exhausted_line` を assert するのは全アームで成立する形だけ**に絞る
        // （`Usage limit reached. You've reached your usage limit. …` は #1096 の動詞
        // `reached your` 経由でしか当たらない —— 旧来の後方互換規則は
        // `usage limit reached` を**小文字のまま**探すので大文字始まりの codex の
        // 文言には当たらない = `TAKO_1096_LEGACY=1` では読めない）
        let codex_limit_lines = [
            "You've hit your usage limit.",
            "You've hit your usage limit. To get more access now, send a request to your admin",
            "Usage limit reached. You've reached your usage limit. Increase your limits to continue using codex.",
            "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits",
        ];
        // #1107 が守るべき不変条件（env に依らない）: 上限を阻害へ倒さない
        for line in codex_limit_lines {
            assert!(
                !entitlement_block_line(line),
                "上限を阻害へ倒している: {line}"
            );
        }
        // 現行規則では 4 本とも上限として読める
        for line in codex_limit_lines {
            if legacy_session_limit() || legacy_out_of_usage() {
                continue; // legacy アームは #1093 / #1096 の A/B が担保する
            }
            assert!(
                is_limit_exhausted_line(line),
                "上限として読めていない: {line}"
            );
        }
    }

    #[test]
    fn issue1106_時間で解けない阻害の8文言を別種として読む() {
        for line in ENTITLEMENT_LINES {
            assert!(
                entitlement_block_line(line),
                "時間で解けない阻害として読めていない: {line}"
            );
        }
        // 画面では前置きに罫線やインデントが付く（実採取の形）
        assert!(entitlement_block_line(
            "  ⎿  Your usage allocation has been disabled by your admin"
        ));
    }

    #[test]
    fn issue1106_時間で解けない阻害は上限停止と混ざらない() {
        // **混ぜると自動復帰（#813）が「解除まで待つ」で撃ち始める**。
        // 判定が排他であることを固定しておけば `detect_worker_error` の
        // 検査順に依らず種別が一意に決まる
        for line in ENTITLEMENT_LINES {
            assert!(
                !is_limit_exhausted_line(line),
                "時間で解けない阻害を上限停止（wait_reset）と読んでいる: {line}"
            );
            // 枠の使用率とは無関係なのでメーターは `--` のまま
            assert_eq!(exhausted_limit_window(line), None, "{line}");
        }
    }

    #[test]
    fn issue1106_時間で解ける上限や警告を阻害と読まない() {
        for line in [
            // #1093 / #1096 で受けているぶん（時間で解ける = wait_reset が正しい）
            "You've hit your session limit · resets 7:50pm (Asia/Tokyo)",
            "You've reached your weekly limit · resets 7:50pm (Asia/Tokyo)",
            "You're out of usage credits · resets 7:50pm (Asia/Tokyo)",
            "Your org is out of usage · contact your admin",
            // `out of usage credits` を含むので #1096 の規則で既に受けている
            // （Issue #1106 の対象外。ここで二重に受けない）
            "Your organization is out of usage credits. Contact your admin to add more.",
            "Claude usage limit reached. Your limit will reset at 3am.",
            // 警告 / 情報（`fCt` / `mCt`）
            "You've used 75% of your weekly limit",
            "You're close to your usage credit limit",
            "You're now using usage credits · Your weekly limit resets 7:50pm",
            "Now using your usage allocation",
            // 上限を上げる案内（阻害されていない）
            "Your admin can enable extra usage at claude.ai/admin-settings/usage.",
            // 通常の出力・地の文
            "⏺ 実装が完了しました。テストは全て緑です。",
            "The seat type field does not include usage in this schema",
        ] {
            assert!(
                !entitlement_block_line(line),
                "阻害ではない行を阻害と読んでいる: {line}"
            );
        }
    }

    #[test]
    fn issue1096_日付つきの解除時刻を絶対時刻として読む() {
        // 観測は 2026-08-15 00:30 JST（`OBSERVED_LOCAL_MIDNIGHT` の 30 分後）
        let reference = REF_MIDNIGHT_JST + 30 * 60;
        let at = |y: i64, m: i64, d: i64, tod: i64| -> Option<i64> {
            Some(days_from_civil(y, m, d) * 86_400 + tod - i64::from(JST))
        };
        for (text, want) in [
            // claude（24 時間より先 = 日付が前置きされる。分が 0 なら省かれる）
            ("resets Sep 8, 3pm (Asia/Tokyo)", at(2026, 9, 8, 15 * 3600)),
            (
                "resets Sep 8, 3:05pm (Asia/Tokyo)",
                at(2026, 9, 8, 15 * 3600 + 5 * 60),
            ),
            // codex（序数 + 年・年と時刻のあいだにカンマ無し）
            (
                "try again at Aug 28th, 2026 4:24 AM",
                at(2026, 8, 28, 4 * 3600 + 24 * 60),
            ),
            // 週枠の実運用形（7 日先）
            (
                "You've hit your weekly limit · resets Aug 22, 9:15am (Asia/Tokyo)",
                at(2026, 8, 22, 9 * 3600 + 15 * 60),
            ),
        ] {
            assert_eq!(
                parse_reset_at(text, reference, JST),
                want,
                "日付つきの解除時刻が読めていない: {text}"
            );
        }
    }

    #[test]
    fn issue1096_日付つきは24時間より先でも丸めない() {
        let reference = REF_MIDNIGHT_JST + 30 * 60;
        let got = parse_reset_at("resets Sep 8, 3pm (Asia/Tokyo)", reference, JST).expect("読める");
        // #1096 前は「次に来る 15:00」= 観測日の 15:00 へ丸まっていた（24 日早い）
        let rounded = REF_MIDNIGHT_JST + 15 * 3600;
        assert_ne!(got, rounded, "24 時間以内へ丸めてしまっている");
        assert_eq!(
            (got - reference) / 86_400,
            24,
            "8/15 00:30 から 9/8 15:00 まで 24 日と少しあるはず"
        );
    }

    #[test]
    fn issue1096_日付なしの表記は従来どおり次に来る同じ時刻() {
        // 回帰: 日付が無い形の解釈は 1 ビットも変えない（24 時間上限も維持）
        let reference = REF_MIDNIGHT_JST + 30 * 60;
        assert_eq!(
            parse_reset_at("resets 7:50pm (Asia/Tokyo)", reference, JST),
            Some(REF_MIDNIGHT_JST + 19 * 3600 + 50 * 60)
        );
        // 観測より前の時刻は翌日
        assert_eq!(
            parse_reset_at("Your limit will reset at 12:10am", reference, JST),
            Some(REF_MIDNIGHT_JST + 86_400 + 10 * 60)
        );
    }

    #[test]
    fn issue1096_実在しない日付や範囲外は不明として扱う() {
        let reference = REF_MIDNIGHT_JST + 30 * 60;
        for text in [
            // 実在しない日（往復検算で弾く）
            "resets Feb 31, 3pm",
            // `resets at ` は `resets ` を含む。別のアンカーで読み直して
            // 前方走査に `3pm` を拾わせてはいけない（判断が骨抜きになる）
            "resets at Feb 31, 3pm",
            // 60 日より先（誤パースとみなす）
            "resets Dec 31, 2028, 3pm",
            // 月名があっても時刻が無い
            "Try again at Aug 28th, 2026.",
            // 月名も時刻も無い
            "Try again at some point next week.",
        ] {
            assert_eq!(
                parse_reset_at(text, reference, JST),
                None,
                "不明に落ちていない: {text}"
            );
        }
    }

    #[test]
    fn issue1096_年をまたぐ日付は最も近い年を採る() {
        // 12/28 に観測して `resets Jan 3, 9am` なら**翌年**の 1/3（前年ではない）
        let dec28 = days_from_civil(2026, 12, 28) * 86_400 - i64::from(JST) + 10 * 3600;
        assert_eq!(
            parse_reset_at("resets Jan 3, 9am", dec28, JST),
            Some(days_from_civil(2027, 1, 3) * 86_400 + 9 * 3600 - i64::from(JST))
        );
        // 年が変わるときは claude が `year` を入れる（`{year:"numeric"}`）ので、
        // 明示された年をそのまま採る
        assert_eq!(
            parse_reset_at("resets Jan 3, 2027, 9:15am (Asia/Tokyo)", dec28, JST),
            Some(days_from_civil(2027, 1, 3) * 86_400 + 9 * 3600 + 15 * 60 - i64::from(JST))
        );
    }

    #[test]
    fn issue1096_日付つき表記は組んで読み直せる() {
        // fixture 生成（`format_dated_reset`）とパーサが同じ書式理解を共有していることを
        // 往復で固定する。ずれると「未来の日付を作ったのに読めない」形の fixture ができる
        let base = REF_MIDNIGHT_JST;
        for (delta_days, tod) in [
            (2, 4 * 3600 + 24 * 60),
            (7, 9 * 3600 + 15 * 60),
            (21, 0),         // 00:00 = 12 AM
            (31, 12 * 3600), // 12:00 = 12 PM
            (3, 23 * 3600 + 59 * 60),
        ] {
            let at = base + delta_days * 86_400 + tod;
            let text = format_dated_reset(at, JST);
            let parsed = parse_reset_at(&format!("try again at {text}"), base, JST);
            assert_eq!(parsed, Some(at), "往復しない: {text}");
        }
        // **精度は分まで**（書式に秒が無い）。秒を含む値は分へ丸めて戻る
        let with_secs = base + 5 * 86_400 + 4 * 3600 + 24 * 60 + 37;
        let text = format_dated_reset(with_secs, JST);
        assert_eq!(
            parse_reset_at(&format!("try again at {text}"), base, JST),
            Some(with_secs - 37),
            "秒は落ちる（期待値も分へ丸める必要がある）: {text}"
        );
        // 序数の綴り（11th / 21st / 22nd / 23rd / 13th）
        for (d, want) in [
            (1, "1st"),
            (2, "2nd"),
            (3, "3rd"),
            (11, "11th"),
            (21, "21st"),
        ] {
            let at = days_from_civil(2026, 12, d) * 86_400 - i64::from(JST) + 9 * 3600;
            assert!(
                format_dated_reset(at, JST).starts_with(&format!("Dec {want},")),
                "序数が違う: {}",
                format_dated_reset(at, JST)
            );
        }
    }

    #[test]
    fn issue1096_暦の変換は往復する() {
        // 自前の `days_from_civil` / `civil_from_days`（外部クレートを増やさない）
        for (y, m, d) in [
            (1970, 1, 1),
            (2000, 2, 29),
            (2026, 8, 15),
            (2026, 12, 31),
            (2027, 1, 1),
            (2100, 3, 1),
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d), "{y}-{m}-{d}");
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
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
    /// 早すぎる再開を 3 回撃って諦めていた（= 上限が解けても朝まで止まったまま）。
    ///
    /// **#1096 で期待値を直した**: #985 は時刻だけを前方走査で拾って
    /// 「次に来る同じ時刻」へ丸めていたので、この 3 例の期待値は観測日（2026-08-15）の
    /// 04:24 —— fixture が書いている日付より **13 / 13 / 19 日早い値**を固定していた。
    /// つまり #985 のテスト自体が早撃ちを正としていた。いまは書かれた日付を採る
    #[test]
    fn issue985_codexの日付つきリセット時刻を読む() {
        let reference = REF_MIDNIGHT_JST + 30 * 60; // 00:30 JST に観測
        let at = |y: i64, m: i64, d: i64| -> Option<i64> {
            Some(days_from_civil(y, m, d) * 86_400 + 4 * 3600 + 24 * 60 - i64::from(JST))
        };
        for (text, want, days_early_before) in [
            (
                "■ You've hit your usage limit. ... try again at Aug 28th, 2026 4:24 AM.",
                at(2026, 8, 28),
                13,
            ),
            (
                "You've hit your usage limit. Try again at Aug 28, 2026 4:24 AM.",
                at(2026, 8, 28),
                13,
            ),
            (
                "Try again at Sep 3rd, 2026 4:24 AM or try again later.",
                at(2026, 9, 3),
                19,
            ),
        ] {
            assert_eq!(
                parse_reset_at(text, reference, JST),
                want,
                "日付を挟んだ時刻が読めない: {text}"
            );
            // 検算: #1096 前の期待値（観測日の 04:24）からどれだけ早かったか
            let before = REF_MIDNIGHT_JST + 4 * 3600 + 24 * 60;
            assert_eq!(
                (want.expect("上で検査済み") - before) / 86_400,
                days_early_before,
                "fixture の日付と期待値が合っていない: {text}"
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

    // --- #1123: ペイン幅で折り返された見出し ---

    /// 実採取（2026-09-04。**幅 25 桁**のペインで worker 4 体が止まっていた画面）。
    /// claude 2.1.258 が自分で語の境界で折り返し、続きを内容の列（5 桁）へ字下げする。
    /// どの物理行も #1093 / #1096 の規則に当たらないので、自動復帰も watch の
    /// `WORKER_ERROR` も同時に外れていた（解除後 7.5 時間の停止）
    const WRAPPED_25: [&str; 7] = [
        "  ⎿  You've hit your",
        "     session limit ·",
        "     resets 5:50am",
        "     (Asia/Tokyo)",
        "     /usage-credits to",
        "     request more usage",
        "     from your admin.",
    ];

    /// 折り返す前の 1 論理行（80 桁のペインならこの形で 1 行に収まる）
    const WRAPPED_SOURCE: &str = "You've hit your session limit · resets 5:50am (Asia/Tokyo) \
                                  /usage-credits to request more usage from your admin.";

    /// claude TUI の折り返しを再現する（先頭行は目印つき・続きは内容の列へ字下げ）。
    /// 実採取（[`WRAPPED_25`]）と形が一致することを別のテストで固定してあるので、
    /// 他の幅もこれで作ってよい
    fn wrap_claude_block(text: &str, cols: usize) -> Vec<String> {
        let (first, cont) = ("  ⎿  ", "     ");
        let mut out: Vec<String> = Vec::new();
        let mut line = first.to_string();
        let mut width = first.chars().count();
        for word in text.split(' ') {
            let w = word.chars().count();
            let head = out.is_empty() && line.chars().count() == first.chars().count();
            if !head && width + 1 + w > cols {
                out.push(line);
                line = format!("{cont}{word}");
                width = cont.chars().count() + w;
            } else if head {
                line.push_str(word);
                width += w;
            } else {
                line.push(' ');
                line.push_str(word);
                width += 1 + w;
            }
        }
        out.push(line);
        out
    }

    #[test]
    fn issue1123_折り返しの生成器が実採取と一致する() {
        // 生成器を実採取に固定しておく。ここがずれたら他の幅のテストも信用できない
        assert_eq!(
            wrap_claude_block(WRAPPED_SOURCE, 25),
            WRAPPED_25.to_vec(),
            "25 桁の折り返しが 2026-09-04 の実採取と一致しない"
        );
    }

    #[test]
    fn issue1123_実採取の折り返し見出しを結合して上限として読む() {
        // 前提: **物理行はどれも当たらない**（これが #1123 の実害そのもの）
        for l in WRAPPED_25 {
            assert!(
                !is_limit_exhausted_line(l),
                "物理行が単体で当たるなら #1123 の前提が違う: {l}"
            );
        }
        let joined = unwrap_wrapped_lines(&WRAPPED_25);
        assert_eq!(joined.len(), 1, "1 本の論理行へ戻らない: {joined:?}");
        assert!(
            is_limit_exhausted_line(&joined[0]),
            "結合しても上限として読めない: {}",
            joined[0]
        );
        assert_eq!(
            exhausted_limit_window(&joined[0]),
            Some(LimitWindow::FiveHour),
            "`session limit` は 5h 枠（メーターを 100% にする根拠）"
        );
        // 解除時刻（`resets 5:50am`）も同じ 1 本から読める
        let observed = 1_786_752_000 - 9 * 3600; // JST 09:00 相当
        assert!(
            parse_reset_at(&joined[0], observed, 9 * 3600).is_some(),
            "結合後の行から解除時刻が読めない: {}",
            joined[0]
        );
    }

    #[test]
    fn issue1123_幅を変えても見出しを読む() {
        // 実発生は 21〜25 桁。80 桁は折り返しが起きない側の対照
        for cols in [21usize, 25, 40, 80] {
            let lines = wrap_claude_block(WRAPPED_SOURCE, cols);
            let joined = unwrap_wrapped_lines(&lines);
            assert_eq!(joined.len(), 1, "{cols} 桁で 1 本へ戻らない: {joined:?}");
            assert!(
                is_limit_exhausted_line(&joined[0]),
                "{cols} 桁で上限として読めない: {}",
                joined[0]
            );
            assert_eq!(
                exhausted_limit_window(&joined[0]),
                Some(LimitWindow::FiveHour),
                "{cols} 桁で枠が読めない"
            );
        }
    }

    #[test]
    fn issue1123_続きの字下げが塊の列でも結合する() {
        // 実採取（同じ 25 桁の画面）。`⎿` の塊なのに続きが**内容の列ではなく
        // 塊の列**（2 桁）へ字下げされることがある。だから幅の一致では判定しない
        let lines = [
            "  ⎿  You have hit",
            "  your session",
            "  limit · resets",
            "  5:50am",
        ];
        let joined = unwrap_wrapped_lines(&lines);
        assert_eq!(
            joined.len(),
            1,
            "字下げ 2 桁の続きが結合されない: {joined:?}"
        );
        assert!(is_limit_exhausted_line(&joined[0]), "{}", joined[0]);
    }

    #[test]
    fn issue1123_目印の字下げが無くても続きは結合する() {
        // 塊の先頭行が 0 桁目から始まる形（`⎿` に字下げが付かない描画）。
        // 先頭行は「字下げ無し = 新しい塊」で始まり、続きは字下げがあるので結合される
        let lines = [
            "⎿  You've hit your",
            "   session limit ·",
            "   resets 5:50am",
        ];
        let joined = unwrap_wrapped_lines(&lines);
        assert_eq!(joined.len(), 1, "{joined:?}");
        assert!(is_limit_exhausted_line(&joined[0]), "{}", joined[0]);
    }

    #[test]
    fn issue1123_字下げの無い行は続きにしない() {
        // 字下げは「折り返しの続き」の唯一の手がかり（claude は続きを必ず字下げする）。
        // 0 桁目から始まる行までつなげると、無関係な段落から偽の見出しが生まれる
        let lines = ["⎿  You've hit your", "session limit · resets 5:50am"];
        let joined = unwrap_wrapped_lines(&lines);
        assert_eq!(
            joined.len(),
            2,
            "字下げの無い行を続きにしている: {joined:?}"
        );
        assert!(joined.iter().all(|l| !is_limit_exhausted_line(l)));
    }

    #[test]
    fn issue1123_折り返しが無ければ論理行は入力そのまま() {
        // 目印つきの行・字下げ無しの行だけの画面では結合が 1 度も起きない
        let lines = [
            "⏺ 実装を進めます",
            "  ⎿  You've hit your session limit · resets 7:50pm (Asia/Tokyo)",
            "",
            "────────────────────────",
            "❯ ",
            "────────────────────────",
        ];
        let joined = unwrap_wrapped_lines(&lines);
        let want: Vec<String> = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim_end().to_string())
            .collect();
        assert_eq!(joined, want, "折り返しの無い画面で行が変わっている");
    }

    #[test]
    fn issue1123_空行をまたいで結合しない() {
        // 空行は塊の切れ目。またいでつなぐと無関係な段落から偽の見出しが生まれる
        let lines = ["  ⎿  You've hit your", "", "     session limit ·"];
        let joined = unwrap_wrapped_lines(&lines);
        assert_eq!(joined.len(), 2, "空行をまたいで結合している: {joined:?}");
        assert!(joined.iter().all(|l| !is_limit_exhausted_line(l)));
    }

    #[test]
    fn issue1123_自動モデル切替の告知は折り返しても上限にしない() {
        // `limit reached, now using …` は worker が止まらない告知（#157 の除外条件）。
        // 折り返しを結合すると除外条件のほうが当たるので、結合しても上限にならない
        let lines = wrap_claude_block("Opus limit reached, now using Sonnet", 21);
        for l in unwrap_wrapped_lines(&lines) {
            assert!(!is_limit_exhausted_line(&l), "自動切替を上限と読んだ: {l}");
        }
    }

    #[test]
    fn issue1123_時間で解けない阻害は折り返しても阻害のまま() {
        // #1106 / #1107 の排他は結合後も保たれる（混ぜると自動復帰が空撃ちする）
        for text in [
            "Your seat type doesn't include usage credits",
            "Your group's usage limit is set to $0",
            "You've reached your workspace credit limit",
        ] {
            let joined = unwrap_wrapped_lines(&wrap_claude_block(text, 21));
            assert_eq!(joined.len(), 1, "{text}: {joined:?}");
            assert!(
                entitlement_block_line(&joined[0]),
                "阻害として読めない: {}",
                joined[0]
            );
            assert!(
                !is_limit_exhausted_line(&joined[0]),
                "阻害を上限と読んだ（排他が破れている）: {}",
                joined[0]
            );
        }
    }

    #[test]
    fn issue1123_警告や情報は折り返しても上限にしない() {
        // まだ動けるペインへナッジを撃たないための境界（#1096 と同じ分類）
        for text in [
            "You've used 90% of your session limit",
            "You're close to your weekly limit",
            "You're now using usage credits",
        ] {
            let joined = unwrap_wrapped_lines(&wrap_claude_block(text, 21));
            for l in &joined {
                assert!(!is_limit_exhausted_line(l), "{text} を上限と読んだ: {l}");
            }
        }
    }

    #[test]
    fn issue1123_結合は行数と文字数で歯止めをかける() {
        // 字下げが続くだけの領域を丸ごと 1 本へ畳まない
        let many: Vec<String> = (0..40).map(|i| format!("     line{i}")).collect();
        let joined = unwrap_wrapped_lines(&many);
        assert!(joined.len() > 1, "40 行が 1 本に畳まれた");
        assert!(
            joined[0].split(' ').filter(|s| !s.is_empty()).count() <= MAX_WRAP_JOIN_LINES + 1,
            "行数の歯止めが効いていない: {}",
            joined[0]
        );
        let long: Vec<String> = (0..30)
            .map(|_| format!("     {}", "x".repeat(80)))
            .collect();
        for l in unwrap_wrapped_lines(&long) {
            assert!(
                l.chars().count() <= MAX_WRAP_JOIN_CHARS + 80,
                "文字数の歯止めが効いていない: {}",
                l.chars().count()
            );
        }
    }

    #[test]
    fn issue1123_末尾から順に返す() {
        // 下の行ほど新しいので、フォールバックの走査も末尾から
        let lines = ["⏺ a", "⏺ b", "⏺ c"];
        assert_eq!(
            unwrapped_tail(&lines, 2),
            vec!["⏺ c".to_string(), "⏺ b".into()]
        );
    }
}

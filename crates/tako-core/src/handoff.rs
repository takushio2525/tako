//! master の自動ハンドオフ（Issue #749）。
//!
//! master のコンテキストが閾値を超えたら、**新しい master に乗り換える**。
//! `/compact` の自動実行は「明らかに話が通じなくなる」ため採らない（ユーザー方針）。
//!
//! このモジュールが持つのは**判定と文面だけ**（GPUI / ファイル I/O 非依存）。
//! - 閾値の値域と丸め（`clamp_ctx_threshold` / `parse_ctx_threshold`）
//! - 「今ナッジを送るべきか」の純粋関数（`nudge_decision`）
//! - master へ送る文面（`nudge_prompt` / `successor_prompt`）
//!
//! 実際の送信は tako-app の定期 tick（画面由来の ctx% を持っている層）が、
//! 新 master の spawn は `tako-control::dispatch` が担う。

use std::time::Duration;

use crate::i18n::{lang, Lang};

/// ユーザーが設定できる ctx 閾値の下限（%）
pub const CTX_THRESHOLD_MIN: u32 = 50;
/// ユーザーが設定できる ctx 閾値の上限（%）
pub const CTX_THRESHOLD_MAX: u32 = 60;
/// 既定の ctx 閾値（%）
pub const CTX_THRESHOLD_DEFAULT: u32 = 60;

/// 閾値を設定可能な範囲へ丸める。
/// 手書きの config.yaml に範囲外の値が入っていても発動判定を壊さないための保険
pub fn clamp_ctx_threshold(v: u32) -> u32 {
    v.clamp(CTX_THRESHOLD_MIN, CTX_THRESHOLD_MAX)
}

/// CLI / MCP から受けた閾値を検証する。範囲外は**黙って丸めず**エラーにする
/// （設定したつもりの値と実際が食い違うのを防ぐ）
pub fn parse_ctx_threshold(v: u32) -> Result<u32, String> {
    if (CTX_THRESHOLD_MIN..=CTX_THRESHOLD_MAX).contains(&v) {
        Ok(v)
    } else {
        Err(format!(
            "ctx_threshold は {CTX_THRESHOLD_MIN}〜{CTX_THRESHOLD_MAX} の範囲で指定する: {v}"
        ))
    }
}

/// role 文字列から master のプロファイル名を取り出す。
/// `orchestrator-master` → `"default"` / `orchestrator-master:<name>` → `"<name>"`。
/// worker / solo / それ以外は None（自動ハンドオフの対象は master だけ）
pub fn master_profile_of_role(role: &str) -> Option<&str> {
    let rest = role.strip_prefix("orchestrator-master")?;
    if rest.is_empty() {
        Some("default")
    } else {
        rest.strip_prefix(':').filter(|s| !s.is_empty())
    }
}

/// 前回のナッジが無視されたときの再送間隔
pub const NUDGE_REPEAT: Duration = Duration::from_secs(600);
/// ペイン起動直後の猶予（復元直後の画面残渣で誤爆しないための待ち）
pub const NUDGE_GRACE: Duration = Duration::from_secs(60);
/// 同一ペインへ送るナッジの上限。これを超えたら黙る（無限に文脈を食わない）
pub const NUDGE_MAX: u32 = 3;

/// ナッジ判定の入力（すべて呼び出し側で観測できる値）
#[derive(Debug, Clone)]
pub struct NudgeInput {
    /// プロファイルの `auto_handoff`（false なら一切送らない）
    pub auto_handoff: bool,
    /// 画面から読んだ ctx 使用率（%）。取れないときは None
    pub ctx_percent: Option<u32>,
    /// 解決済みの閾値（%）
    pub threshold: u32,
    /// このペインが観測されはじめてからの経過時間
    pub pane_age: Duration,
    /// 直近のナッジからの経過時間（まだ送っていなければ None）
    pub since_last_nudge: Option<Duration>,
    /// このペインへ送ったナッジの回数
    pub sent_count: u32,
    /// このペインから handoff がすでに実行済み（新 master が立っている）
    pub handoff_started: bool,
}

/// ナッジを送らない理由（診断・ログ用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeSkip {
    /// プロファイルで自動ハンドオフが無効
    Disabled,
    /// 画面から ctx% が読めない（TUI が出ていない・起動途中）
    NoCtxData,
    /// 閾値未満
    BelowThreshold,
    /// 起動直後の猶予中
    WithinGrace,
    /// 前回のナッジから再送間隔が経っていない
    RepeatTooSoon,
    /// 上限まで送った
    MaxReached,
    /// すでに handoff 済み
    HandoffStarted,
}

impl NudgeSkip {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoCtxData => "no_ctx_data",
            Self::BelowThreshold => "below_threshold",
            Self::WithinGrace => "within_grace",
            Self::RepeatTooSoon => "repeat_too_soon",
            Self::MaxReached => "max_reached",
            Self::HandoffStarted => "handoff_started",
        }
    }
}

/// ナッジ判定の結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeDecision {
    /// 今このペインへナッジを送る
    Send,
    /// 送らない（理由つき）
    Skip(NudgeSkip),
}

impl NudgeDecision {
    pub fn should_send(self) -> bool {
        matches!(self, Self::Send)
    }
}

/// 「今ナッジを送るべきか」を判定する。
///
/// busy かどうかは**見ない**。claude は生成中の入力を内部キューへ入れてターン終了時に
/// 処理するので（#572 で実測）、busy 中に送っても割り込みにはならず「区切りの良い
/// タイミング」で届く。逆に busy を待つと、長時間走り続ける master に永遠に届かない
pub fn nudge_decision(input: &NudgeInput) -> NudgeDecision {
    if !input.auto_handoff {
        return NudgeDecision::Skip(NudgeSkip::Disabled);
    }
    if input.handoff_started {
        return NudgeDecision::Skip(NudgeSkip::HandoffStarted);
    }
    let Some(pct) = input.ctx_percent else {
        return NudgeDecision::Skip(NudgeSkip::NoCtxData);
    };
    if pct < input.threshold {
        return NudgeDecision::Skip(NudgeSkip::BelowThreshold);
    }
    if input.pane_age < NUDGE_GRACE {
        return NudgeDecision::Skip(NudgeSkip::WithinGrace);
    }
    if input.sent_count >= NUDGE_MAX {
        return NudgeDecision::Skip(NudgeSkip::MaxReached);
    }
    match input.since_last_nudge {
        Some(elapsed) if elapsed < NUDGE_REPEAT => NudgeDecision::Skip(NudgeSkip::RepeatTooSoon),
        _ => NudgeDecision::Send,
    }
}

/// 閾値超過で master へ送るナッジ文面（Issue #749 要件 2）。
///
/// 短さを優先する: これ自体が master の文脈を食うため、手順は 2 行に畳んでいる。
/// `handoff_path` は書き込み先の絶対パス（不明なら None）
pub fn nudge_prompt(ctx_percent: u32, threshold: u32, handoff_path: Option<&str>) -> String {
    let path = handoff_path.unwrap_or("handoff/<profile>.md");
    match lang() {
        Lang::Ja => format!(
            "【tako 自動通知】コンテキスト使用率が {ctx_percent}%（閾値 {threshold}%）に達しました。\n\
             引き継ぎを開始してください。ユーザーの許可を求める必要はありません。\n\
             1. 引き継ぎファイル `{path}` を今の状況で上書きする\
             （進行中タスク・spawn 済み worker とその pane・未完の判断・次の一手・ユーザーの直近の意図）\n\
             2. `tako_orchestrator_handoff` を呼ぶ（後任 master が同じタブに立ち、\
             引き継ぎを確認してからこのペインを閉じます）\n\
             まだ返しきっていない報告があるなら、それだけ先に片付けてから 1 に進んでください。"
        ),
        Lang::En => format!(
            "[tako auto-notice] Context usage has reached {ctx_percent}% (threshold {threshold}%).\n\
             Start the handoff now. You do not need to ask the user for permission.\n\
             1. Overwrite the handoff file `{path}` with your current state \
             (in-flight tasks, spawned workers and their panes, open decisions, next steps, \
             the user's most recent intent).\n\
             2. Call `tako_orchestrator_handoff`. A successor master starts in this tab, \
             verifies the handoff, and then closes this pane.\n\
             If you owe the user a reply you have not delivered yet, finish that one thing first, \
             then go to step 1."
        ),
    }
}

/// 後任 master へ送る初期プロンプト（Issue #749 要件 3）。
///
/// **kill の順序を文面で固定する**のが要点: 引き継ぎ確認 → 旧ペインの入力欄確認 →
/// kill。確認前に kill させない（前任の未送達指示を取りこぼすと復元不能）
pub fn successor_prompt(
    profile: &str,
    handoff_content: &str,
    previous_pane: Option<u64>,
) -> String {
    let body = match lang() {
        Lang::Ja => format!(
            "あなたは前任 master から引き継ぎを受けた新しい master です。\n\
             以下の引き継ぎファイルの内容を読み、前任の状態を把握してから業務を開始してください。\n\n\
             --- handoff/{profile}.md ---\n\
             {handoff_content}\n\
             --- end ---\n"
        ),
        Lang::En => format!(
            "You are the new master, taking over from your predecessor.\n\
             Read the handoff file below and understand the previous state before starting work.\n\n\
             --- handoff/{profile}.md ---\n\
             {handoff_content}\n\
             --- end ---\n"
        ),
    };
    let steps = match (lang(), previous_pane) {
        (Lang::Ja, Some(pane)) => format!(
            "\n引き継ぎ手順（この順で行う。順序を入れ替えない）:\n\
             1. 引き継ぎファイルの内容と**実態**を突き合わせる。\
             `tako_orchestrator_workers` で spawn 済み worker を、`tako_list_panes` で\
             このタブのペイン構成を確認し、書かれていない worker や消えた worker を把握する。\n\
             2. 把握できたら「引き継ぎ完了」と、実態との食い違い（あれば）を報告する。\n\
             3. 前任 master のペイン（pane {pane}）を `tako_read_pane` で読み、\
             **入力欄にユーザーの未送達の指示が残っていないか**を確認する\
             （`input_status` が user / mixed、または `queued_messages_pending` が true なら残っている）。\
             残っていたらその内容を引き取り、ユーザーへ「これはまだ実行していません」と伝える。\n\
             4. 3 まで終えてから `tako_close_pane` で pane {pane} を閉じる\
             （幽霊 master を残さない）。1〜3 のどれかができていないなら**閉じない**。\n\
             5. 以降はこのペインが master。ユーザーの次の指示を待つ。"
        ),
        (Lang::En, Some(pane)) => format!(
            "\nHandoff procedure (in this order — do not reorder):\n\
             1. Cross-check the handoff file against **reality**: use \
             `tako_orchestrator_workers` for spawned workers and `tako_list_panes` for this \
             tab's pane layout, and note any worker that is missing from the file or gone.\n\
             2. Once you have the picture, report \"handoff complete\" plus any mismatch you found.\n\
             3. Read the predecessor's pane (pane {pane}) with `tako_read_pane` and check \
             **whether the user left an undelivered instruction in its input box** \
             (`input_status` of user / mixed, or `queued_messages_pending` true means there is one). \
             If so, take it over and tell the user it has not been executed yet.\n\
             4. Only after step 3, close pane {pane} with `tako_close_pane` \
             (no ghost masters left behind). If any of steps 1-3 did not succeed, **do not close it**.\n\
             5. From here on this pane is the master. Wait for the user's next instruction."
        ),
        (Lang::Ja, None) => {
            "\n引き継ぎ内容を把握したら「引き継ぎ完了」と報告し、待機してください。".to_string()
        }
        (Lang::En, None) => {
            "\nOnce you have absorbed the handoff, report \"handoff complete\" and stand by."
                .to_string()
        }
    };
    format!("{body}{steps}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> NudgeInput {
        NudgeInput {
            auto_handoff: true,
            ctx_percent: Some(65),
            threshold: 60,
            pane_age: Duration::from_secs(300),
            since_last_nudge: None,
            sent_count: 0,
            handoff_started: false,
        }
    }

    #[test]
    fn 閾値は50から60へ丸められる() {
        assert_eq!(clamp_ctx_threshold(0), 50);
        assert_eq!(clamp_ctx_threshold(49), 50);
        assert_eq!(clamp_ctx_threshold(50), 50);
        assert_eq!(clamp_ctx_threshold(55), 55);
        assert_eq!(clamp_ctx_threshold(60), 60);
        assert_eq!(clamp_ctx_threshold(80), 60);
        assert_eq!(clamp_ctx_threshold(u32::MAX), 60);
    }

    #[test]
    fn 明示指定の範囲外はエラーになる() {
        assert_eq!(parse_ctx_threshold(50), Ok(50));
        assert_eq!(parse_ctx_threshold(60), Ok(60));
        assert!(parse_ctx_threshold(49).is_err());
        assert!(parse_ctx_threshold(61).is_err());
        // 丸めずエラーにするのが要点（設定値と実効値の食い違いを作らない）
        assert!(parse_ctx_threshold(80).unwrap_err().contains("50〜60"));
    }

    #[test]
    fn 閾値超過でナッジを送る() {
        assert_eq!(nudge_decision(&base()), NudgeDecision::Send);
    }

    #[test]
    fn 閾値ちょうどでも送る() {
        let input = NudgeInput {
            ctx_percent: Some(60),
            ..base()
        };
        assert_eq!(nudge_decision(&input), NudgeDecision::Send);
    }

    #[test]
    fn 閾値未満では送らない() {
        let input = NudgeInput {
            ctx_percent: Some(59),
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::BelowThreshold)
        );
    }

    #[test]
    fn 閾値を下げると同じctxでも送るようになる() {
        let low = NudgeInput {
            ctx_percent: Some(52),
            threshold: 50,
            ..base()
        };
        let high = NudgeInput {
            threshold: 60,
            ..low.clone()
        };
        assert_eq!(nudge_decision(&low), NudgeDecision::Send);
        assert_eq!(
            nudge_decision(&high),
            NudgeDecision::Skip(NudgeSkip::BelowThreshold)
        );
    }

    #[test]
    fn 自動ハンドオフ無効なら送らない() {
        let input = NudgeInput {
            auto_handoff: false,
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::Disabled)
        );
    }

    #[test]
    fn ctxが読めないときは送らない() {
        let input = NudgeInput {
            ctx_percent: None,
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::NoCtxData)
        );
    }

    #[test]
    fn 起動直後の猶予中は送らない() {
        let input = NudgeInput {
            pane_age: Duration::from_secs(10),
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::WithinGrace)
        );
    }

    #[test]
    fn 再送間隔前は送らず経過後は送る() {
        let soon = NudgeInput {
            since_last_nudge: Some(Duration::from_secs(60)),
            sent_count: 1,
            ..base()
        };
        assert_eq!(
            nudge_decision(&soon),
            NudgeDecision::Skip(NudgeSkip::RepeatTooSoon)
        );
        let later = NudgeInput {
            since_last_nudge: Some(NUDGE_REPEAT + Duration::from_secs(1)),
            ..soon
        };
        assert_eq!(nudge_decision(&later), NudgeDecision::Send);
    }

    #[test]
    fn 上限まで送ったら黙る() {
        let input = NudgeInput {
            sent_count: NUDGE_MAX,
            since_last_nudge: Some(NUDGE_REPEAT * 10),
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::MaxReached)
        );
    }

    #[test]
    fn handoff済みなら送らない() {
        let input = NudgeInput {
            handoff_started: true,
            ..base()
        };
        assert_eq!(
            nudge_decision(&input),
            NudgeDecision::Skip(NudgeSkip::HandoffStarted)
        );
    }

    #[test]
    fn ナッジ文面に実数値と手順が入る() {
        let s = nudge_prompt(72, 55, Some("/tmp/handoff/default.md"));
        assert!(s.contains("72"), "{s}");
        assert!(s.contains("55"), "{s}");
        assert!(s.contains("/tmp/handoff/default.md"), "{s}");
        assert!(s.contains("tako_orchestrator_handoff"), "{s}");
    }

    #[test]
    fn 後任プロンプトはkillを確認の後に置く() {
        let s = successor_prompt("default", "## 状態\n進行中: なし", Some(42));
        assert!(s.contains("進行中: なし"), "{s}");
        assert!(s.contains("tako_read_pane"), "{s}");
        assert!(s.contains("tako_close_pane"), "{s}");
        assert!(s.contains("42"), "{s}");
        // 順序の構造保証: read（確認）が close（kill）より前に現れる
        let read_at = s.find("tako_read_pane").expect("read 手順がある");
        let close_at = s.find("tako_close_pane").expect("close 手順がある");
        assert!(
            read_at < close_at,
            "確認より先に kill を書いてはいけない: {s}"
        );
    }

    #[test]
    fn 旧ペイン不明ならkillを指示しない() {
        let s = successor_prompt("default", "state", None);
        assert!(!s.contains("tako_close_pane"), "{s}");
    }

    #[test]
    fn roleからプロファイル名を取り出す() {
        assert_eq!(
            master_profile_of_role("orchestrator-master"),
            Some("default")
        );
        assert_eq!(
            master_profile_of_role("orchestrator-master:tako"),
            Some("tako")
        );
        // master 以外は対象外
        assert_eq!(master_profile_of_role("orchestrator-solo"), None);
        assert_eq!(master_profile_of_role("orchestrator-worker"), None);
        assert_eq!(master_profile_of_role("worker:1"), None);
        assert_eq!(master_profile_of_role(""), None);
        // 似た接頭辞に引っかからない
        assert_eq!(master_profile_of_role("orchestrator-master-old"), None);
        assert_eq!(master_profile_of_role("orchestrator-master:"), None);
    }
}

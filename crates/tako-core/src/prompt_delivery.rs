//! prompt_delivery — 送達フローの顛末を**必ず言う**ための語彙と記録方針（Issue #1259）
//!
//! `tako_send_input(await_prompt=true)` は `{"queued": true}` を即返し、実際の送達は
//! 送達フロー（#32 / #790）が背景で回す。#1259 で本番が 2 回踏んだのは
//! **その顛末がどこにも出ない**ことだった。当時のコードで痕跡が残る口は 3 つあり、
//! 後続 send ではその全部が空振りする:
//!
//! 1. worker レジストリ: `record_prompt_delivery_at` は先頭で
//!    `if flow != SpawnPrompt { return Ok(()) }` = **spawn の初回プロンプト専用**。
//!    稼働中 worker への後続 send の未達は 1 バイトも書かれない
//! 2. `eprintln!`: GUI（.app）の stderr は誰も読めない（診断は persist.log が正 =
//!    `.agent/conventions.md`）
//! 3. `persist.log`: 書くのは `delivery::try_peer` / `log_fallback` = **貼り付け段へ
//!    到達した後**だけ。その前（TUI 待ち・入力欄待ち・peer の背景試行待ち・
//!    起動コマンド待ちの保留）で止まると 1 行も残らない
//!
//! だからこのモジュールが「状態」「止まっている理由」「いつ書くか」を 1 箇所で決め、
//! 送達フロー（tako-app）・応答（`tako-control::dispatch`）・CLI の出力が同じ語彙を通る。
//!
//! # 出さないもの
//!
//! ペインの画面内容・送信テキスト・`TAKO_TOKEN` は**理由コードにも文言にも入れない**
//! （AGENTS.md の絶対ルール）。出すのは状態・理由コード・経過秒・ペイン番号だけ。

use crate::i18n::{self, Lang, Text};

/// 送達フローの状態。`tako_read_pane` の `delivery.state` / persist.log で共有する
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// 積まれた直後（まだ 1 tick も回っていない）
    Queued,
    /// 回っているが進めていない（理由は [`Stall`]）
    Waiting,
    /// 送り切った
    Delivered,
    /// 諦めた（理由コードは `Status::outcome`）
    GaveUp,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Waiting => "waiting",
            State::Delivered => "delivered",
            State::GaveUp => "gave_up",
        }
    }

    /// `as_str` の逆（応答 JSON の `delivery.state` から語彙へ戻す）。
    /// 綴りは [`State::as_str`] から引くので両者が食い違わない。未知の綴りは `None`
    pub fn parse(s: &str) -> Option<Self> {
        [
            State::Queued,
            State::Waiting,
            State::Delivered,
            State::GaveUp,
        ]
        .into_iter()
        .find(|state| state.as_str() == s)
    }

    /// **未決着**か（= これから積む送達がこの状態の後ろに並ぶ）。
    ///
    /// [`still_visible`]（決着した顛末をまだ応答へ出すか）とは別の問い。
    /// 決着した顛末は「その後を問う」ために残すが、**新しい送達の応答に
    /// 名乗らせてはいけない**（#1292）
    pub fn is_pending(self) -> bool {
        match self {
            State::Queued | State::Waiting => true,
            State::Delivered | State::GaveUp => false,
        }
    }
}

/// 送達フローが進めない理由。**安定コード**（応答・ログ・テストが名前で参照する）。
///
/// 「なぜ止まっているか」が分かれば master の次の一手が決まる、という基準で分ける:
/// 待てば解ける（`TuiNotReady` / `PeerPending`）・人が触るしかない（`ChoiceDialog`）・
/// tako 側の不具合（`SessionGone`）で対応が変わる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stall {
    /// ペインのセッションが引けない（閉じられた / まだ起動していない）
    SessionGone,
    /// 同じペインの起動コマンド送達（#640）が終わるまで保留している
    HoldForCommandFlow,
    /// 同じペインの先行フローが終わるまで順番待ちしている
    BehindFlow,
    /// エージェント TUI の起動待ち（alt_screen / 入力欄がまだ出ていない）
    TuiNotReady,
    /// 画面に入力欄（`❯` 等）が見つからない
    NoInputBox,
    /// 選択肢ダイアログが入力欄を奪っている（respond で応答する）
    ChoiceDialog,
    /// 信頼ダイアログを承諾している途中
    TrustDialog,
    /// peer 送達（#790）の背景試行の結果待ち
    PeerPending,
    /// peer への**書き込みが始まった後**に応答が返らない（#1259）。
    /// 届いた可能性があるので再送してはいけない（二重投函）
    PeerSendStalled,
    /// claude のメッセージキューからの取り出し待ち（#572）
    QueueDrain,
    /// 貼り付けが入力欄へ反映されるのを待っている
    PasteNotReflected,
    /// Enter を送ったが入力欄に残っているので再送している
    EnterResend,
}

impl Stall {
    /// 応答・ログ・テストが参照する安定コード
    pub fn code(self) -> &'static str {
        match self {
            Stall::SessionGone => "session_gone",
            Stall::HoldForCommandFlow => "hold_for_command_flow",
            Stall::BehindFlow => "behind_flow",
            Stall::TuiNotReady => "tui_not_ready",
            Stall::NoInputBox => "no_input_box",
            Stall::ChoiceDialog => "choice_dialog",
            Stall::TrustDialog => "trust_dialog",
            Stall::PeerPending => "peer_pending",
            Stall::PeerSendStalled => "peer_send_stalled",
            Stall::QueueDrain => "queue_drain",
            Stall::PasteNotReflected => "paste_not_reflected",
            Stall::EnterResend => "enter_resend",
        }
    }

    /// 何を待っているかの説明（日英）
    pub fn note(self) -> Text {
        match self {
            Stall::SessionGone => Text::new(
                "ペインのセッションが引けない（閉じられた / まだ起動していない）",
                "the pane has no session (closed, or not spawned yet)",
            ),
            Stall::HoldForCommandFlow => Text::new(
                "同じペインの起動コマンドが届くまで保留している",
                "held until the pane's launch command is delivered",
            ),
            Stall::BehindFlow => Text::new(
                "同じペインの先行送達が終わるまで順番待ちしている",
                "queued behind an earlier delivery to the same pane",
            ),
            Stall::TuiNotReady => Text::new(
                "エージェント TUI の起動を待っている",
                "waiting for the agent TUI to start",
            ),
            Stall::NoInputBox => Text::new(
                "画面に入力欄が見つからない",
                "no input box is visible on screen",
            ),
            Stall::ChoiceDialog => Text::new(
                "選択肢ダイアログが入力欄を奪っている（respond で応答する）",
                "a choice dialog owns the input box (answer it with respond)",
            ),
            Stall::TrustDialog => Text::new(
                "信頼ダイアログを承諾している途中",
                "accepting the trust dialog",
            ),
            Stall::PeerPending => Text::new(
                "peer 送達の結果を待っている",
                "waiting for the peer delivery attempt to finish",
            ),
            Stall::PeerSendStalled => Text::new(
                "peer への書き込みが始まった後に応答が返らない（届いた可能性があるので再送しない）",
                "the peer write had already begun when it stopped responding, so it may have arrived: do not resend",
            ),
            Stall::QueueDrain => Text::new(
                "メッセージキューからの取り出しを待っている",
                "waiting to recall the queued message",
            ),
            Stall::PasteNotReflected => Text::new(
                "貼り付けが入力欄へ反映されるのを待っている",
                "waiting for the paste to appear in the input box",
            ),
            Stall::EnterResend => Text::new(
                "入力欄に残っているので Enter を再送している",
                "text remains in the input box, resending Enter",
            ),
        }
    }

    /// 待てば解ける見込みがあるか（master が「待つ / 手を出す」を決める材料）
    pub fn transient(self) -> bool {
        match self {
            Stall::TuiNotReady
            | Stall::TrustDialog
            | Stall::PeerPending
            | Stall::QueueDrain
            | Stall::PasteNotReflected
            | Stall::EnterResend
            | Stall::HoldForCommandFlow
            | Stall::BehindFlow => true,
            Stall::SessionGone
            | Stall::NoInputBox
            | Stall::ChoiceDialog
            | Stall::PeerSendStalled => false,
        }
    }
}

/// peer 送達が「書き切ったが受信を確認できなかった」ときの顛末コード（#790）。
/// [`Stall::PeerSendStalled`] と同じく**再送してはいけない**側なので、綴りを
/// 送達フロー側に持たせず正本をここに置く（#1294）
pub const PEER_UNCONFIRMED: &str = "peer_unconfirmed";

/// 送達を確認できたときの顛末コード。keys 経路（貼り付け反映 + 残留消失）と
/// peer 経路（transcript で受信確認）でコードが違うので両方を正本に持つ
pub const VERIFIED: &str = "verified";
/// peer 送達で受信まで確認できたときの顛末コード
pub const DELIVERED: &str = "delivered";

/// 送達の記録に載せる**確からしさ**（Issue #1294）。
///
/// 旧実装は bool（届いた / 届かなかった）の 2 値だった。ところが peer 送達には
/// 「**書き込みが始まった後に確認が取れない**」= 届いた可能性がある、という
/// 第 3 の顛末がある（[`Stall::PeerSendStalled`] / [`PEER_UNCONFIRMED`]）。
/// これを「未達」として記録すると worker レジストリが `undelivered` を返し、
/// supervisor の自動再送（`recover_prompt_undelivered`）が同じ依頼を撃つ ——
/// #790 / #1015 が構造で潰してきた二重投函が、別の口から起きる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// 届いたと言える（貼り付けが入力欄へ反映され残留も消えた / peer が受信を確認した）
    Delivered,
    /// **未達と断定できる**（1 バイトも送っていない / 貼り付けが画面に出なかった）。
    /// 自動再送を撃ってよいのはこれだけ
    Undelivered,
    /// **送ったかもしれない**（書き込みが始まった後に確認が取れない）。
    /// 未達と断定できないので自動再送は撃たない = 画面を見て確かめてから人が送り直す
    Unverified,
}

impl Confidence {
    /// 応答・ログ・テストが参照する安定コード
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Delivered => "delivered",
            Confidence::Undelivered => "undelivered",
            Confidence::Unverified => "unverified",
        }
    }

    /// 自動再送（supervisor の `recover_prompt_undelivered`）を撃ってよいか。
    /// **`Undelivered` だけが true**（「かもしれない」で撃つと二重投函）
    pub fn may_auto_resend(self) -> bool {
        matches!(self, Confidence::Undelivered)
    }
}

/// 顛末コードごとの確からしさ（[`Stall::transient`] と同型の宣言表。Issue #1294）。
///
/// **既定は未達側**（`Undelivered`）にしてある: 判断のつかない新しい顛末を
/// 「届いたかも」へ倒すと、本当に届いていない worker が救済されなくなる。
/// 再送禁止側（`Unverified`）へ入れてよいのは「**1 バイト以上書いた後**に
/// 確認が取れない」と言える顛末だけ。
///
/// この表が「送達フローの宣言」と「レジストリの記録」の唯一の接点で、
/// 記録側（`record_prompt_delivery`）と読み出し側
/// （`prompt_delivery_assessment_with`）が同じ関数を引くので食い違わない
pub fn outcome_confidence(outcome: &str) -> Confidence {
    if outcome == VERIFIED || outcome == DELIVERED {
        return Confidence::Delivered;
    }
    // 書き込みが始まった後に確認が取れなかった 2 系統。届いた可能性がある
    if outcome == Stall::PeerSendStalled.code() || outcome == PEER_UNCONFIRMED {
        return Confidence::Unverified;
    }
    Confidence::Undelivered
}

/// `TAKO_1294_LEGACY=1` で **#1294 前**の挙動へ戻す（A/B の入口）。
///
/// 戻るのは 1 点: 「送ったかもしれない」もレジストリでは未達（`undelivered`）と
/// 断定し、supervisor の自動再送が撃たれる（= 二重投函）
pub fn legacy_undelivered() -> bool {
    std::env::var_os("TAKO_1294_LEGACY").is_some()
}

/// 送達フローの顛末。`tako_send_input` の応答 / `tako_read_pane` の `delivery` /
/// `persist.log` の 1 行がすべてここから作られる（同じ語彙が 3 経路へ出る）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// 止まっている理由（`Waiting` と、理由を保ったまま諦めた `GaveUp` に入る）
    pub stall: Option<Stall>,
    /// フローを積んでからの経過秒
    pub elapsed_secs: u32,
    /// 決着の理由コード（`Delivered` / `GaveUp` のとき）
    pub outcome: Option<String>,
    /// 送り切った経路（`peer` / `keys`）
    pub transport: Option<&'static str>,
}

impl Status {
    /// 積んだ直後（`tako_send_input` が即返す応答に載る）
    pub fn queued() -> Self {
        Self {
            state: State::Queued,
            stall: None,
            elapsed_secs: 0,
            outcome: None,
            transport: None,
        }
    }

    pub fn waiting(stall: Stall, elapsed_secs: u32) -> Self {
        Self {
            state: State::Waiting,
            stall: Some(stall),
            elapsed_secs,
            outcome: None,
            transport: None,
        }
    }

    pub fn delivered(transport: &'static str, outcome: &str, elapsed_secs: u32) -> Self {
        Self {
            state: State::Delivered,
            stall: None,
            elapsed_secs,
            outcome: Some(outcome.to_string()),
            transport: Some(transport),
        }
    }

    /// 諦めた。`stall` は**諦めた瞬間に待っていたもの**を残す
    /// （「なぜ 120 秒も進めなかったのか」は理由コードだけでは分からない）
    pub fn gave_up(outcome: &str, stall: Option<Stall>, elapsed_secs: u32) -> Self {
        Self {
            state: State::GaveUp,
            stall,
            elapsed_secs,
            outcome: Some(outcome.to_string()),
            transport: None,
        }
    }

    /// `persist.log` / ログの重複判定に使うキー（経過秒を含めない = 同じ状態で
    /// 待っているあいだは同じキーになる）
    pub fn log_key(&self) -> String {
        format!(
            "{}/{}/{}",
            self.state.as_str(),
            self.stall.map(Stall::code).unwrap_or("-"),
            self.outcome.as_deref().unwrap_or("-")
        )
    }

    /// `persist.log` の 1 行。**画面内容・送信テキストは含めない**（絶対ルール）。
    ///
    /// `pane` が None なのは tmux セッション経路（ペインが解決できない送達）。
    /// ペイン番号を持たないので `-` と書き、突き合わせはセッション名で行う
    pub fn log_line(&self, pane: Option<u64>, session: Option<&str>) -> String {
        let pane_label = pane.map_or_else(|| "-".to_string(), |p| p.to_string());
        let mut s = format!(
            "送達フロー: pane={pane_label} 状態={} 経過={}s",
            self.state.as_str(),
            self.elapsed_secs
        );
        if let Some(session) = session {
            // ペイン番号を持たない経路（tmux セッション送達）はここで突き合わせる
            s.push_str(&format!(" session={session}"));
        }
        if let Some(stall) = self.stall {
            s.push_str(&format!(" 理由={}", stall.code()));
        }
        if let Some(outcome) = &self.outcome {
            s.push_str(&format!(" 顛末={outcome}"));
        }
        if let Some(transport) = self.transport {
            s.push_str(&format!(" 経路={transport}"));
        }
        s
    }

    /// 応答用 JSON。`ssh_connect`（#1010）と同じ形にそろえる:
    /// 状態 + 理由コード + 人が読む注記 + 経過秒
    pub fn to_json(&self) -> serde_json::Value {
        self.to_json_in(i18n::lang())
    }

    /// 言語を明示しての JSON（言語グローバルに触らないテスト用。`Note::text_in` と同規約）
    pub fn to_json_in(&self, lang: Lang) -> serde_json::Value {
        serde_json::json!({
            "state": self.state.as_str(),
            "reason": self.stall.map(Stall::code),
            "note": self.stall.map(|s| s.note().text_in(lang).to_string()),
            "transient": self.stall.map(Stall::transient),
            "elapsed_secs": self.elapsed_secs,
            "outcome": self.outcome,
            "transport": self.transport,
        })
    }
}

/// `persist.log` をいつ書くか（#1258 の `ssh_detect::SshSkipLog` と同型）。
///
/// 送達フローは 500ms tick で回るので、毎 tick 書くと 1 分で 120 行積む。
/// **状態が変わったとき**と、変わらないまま [`Journal::HEARTBEAT_SECS`] 経ったときだけ書く
/// （「まだ待っている」も定期的に言わないと、無音との区別がつかない）
#[derive(Debug, Default)]
pub struct Journal {
    last_key: Option<String>,
    last_at: Option<u32>,
}

impl Journal {
    /// 同じ状態で待ち続けているときに言い直す間隔（秒）
    pub const HEARTBEAT_SECS: u32 = 15;

    /// この状態を今書くべきか。書くと決めたら記憶を更新する
    pub fn should_log(&mut self, status: &Status) -> bool {
        let key = status.log_key();
        let changed = self.last_key.as_deref() != Some(key.as_str());
        let heartbeat = self
            .last_at
            .is_none_or(|at| status.elapsed_secs.saturating_sub(at) >= Self::HEARTBEAT_SECS);
        if changed || heartbeat {
            self.last_key = Some(key);
            self.last_at = Some(status.elapsed_secs);
            return true;
        }
        false
    }
}

/// peer 送達（#790）の背景試行がどこまで進んだか。
///
/// **1 バイトも送っていないと言える段階**（`Resolving`）だけが従来経路へ落ちてよい
/// 段階（送った後に落ちると同じ指示が 2 回届く = #790 の不変条件）。
/// だから待ちに上限をつけるには、上限だけでなく**この段階**が必要になる
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerPhase {
    /// 宛先の解決中（`ps` / `tmux list-panes` を叩く。まだ 1 バイトも送っていない）
    #[default]
    Resolving,
    /// 受信箱へ書き込んでいる（落ちてはならない）
    Sending,
    /// 書き切って受信確認を待っている（落ちてはならない）
    Verifying,
}

impl PeerPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            PeerPhase::Resolving => "resolving",
            PeerPhase::Sending => "sending",
            PeerPhase::Verifying => "verifying",
        }
    }
}

/// peer 送達の背景試行の共有状態（段階 + 取り消し）を **1 つの原子**で持つ（#1259）。
///
/// 「上限で諦めてキー経路へ落ちる」と「1 バイト目を書く」は競合する。段階を別に持って
/// 「いま `Resolving` だから落ちてよい」と読むだけでは、**読んだ直後に背景スレッドが
/// 書き始める**ので同じ指示が 2 回届く（#790 の不変条件が崩れる）。
/// どちらか一方だけが勝つように、同じ原子への CAS で決める:
///
/// - 背景スレッド: `Resolving → Sending` に成功したときだけ書き込む
/// - UI スレッド: `Resolving → Cancelled` に成功したときだけキー経路へ落ちる
#[derive(Debug)]
pub struct PeerAttemptState(std::sync::atomic::AtomicU8);

impl Default for PeerAttemptState {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerAttemptState {
    const RESOLVING: u8 = 0;
    const SENDING: u8 = 1;
    const VERIFYING: u8 = 2;
    const CANCELLED: u8 = 3;

    pub fn new() -> Self {
        Self(std::sync::atomic::AtomicU8::new(Self::RESOLVING))
    }

    /// 診断用の段階名（`cancelled` を含む）
    pub fn as_str(&self) -> &'static str {
        match self.0.load(std::sync::atomic::Ordering::SeqCst) {
            Self::SENDING => "sending",
            Self::VERIFYING => "verifying",
            Self::CANCELLED => "cancelled",
            _ => "resolving",
        }
    }

    /// 書き込みを始めてよいか（`Resolving → Sending`）。
    /// false = UI が先に諦めてキー経路へ落ちた = **1 バイトも書いてはならない**
    pub fn begin_sending(&self) -> bool {
        self.0
            .compare_exchange(
                Self::RESOLVING,
                Self::SENDING,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
    }

    /// 書き切った（以降は落ちてはならない）
    pub fn begin_verifying(&self) {
        self.0
            .store(Self::VERIFYING, std::sync::atomic::Ordering::SeqCst);
    }

    /// 上限で諦める（`Resolving → Cancelled`）。
    /// true = まだ 1 バイトも送っていないと言える = キー経路へ落ちてよい
    pub fn cancel_before_send(&self) -> bool {
        self.0
            .compare_exchange(
                Self::RESOLVING,
                Self::CANCELLED,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
    }
}

/// peer の背景試行を待ち続けるかどうかの判断
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerWait {
    /// まだ待つ
    KeepWaiting,
    /// 諦めて従来のキー操作経路へ落ちる（**まだ 1 バイトも送っていない**）
    FallbackToKeys,
    /// 送った後に返らなくなった。二重投函になるので**落ちずに**未確認で決着させる
    StopUnconfirmed,
}

/// peer の背景試行を待つ上限（秒）。
///
/// `delivery::try_peer` の内訳は「宛先の解決 + 書き込み 5 秒 + 受信確認 6 秒」で、
/// 素なら 12 秒あれば返る。ところが解決は `ps` と `tmux list-panes -a` を
/// **上限なしの `.output()`** で叩くので、器が詰まると返らない
/// （#1259 の実機には残骸 tmux サーバーが 2299 個あった）。
/// 上限は素の所要より十分広く、フロー全体の 120 秒より十分狭く採る
/// （落ちたあとキー操作経路が完走する余地を残す）
pub const PEER_BUDGET_SECS: u32 = 25;

/// 送達フロー 1 本の総合タイムアウト（秒。#32 以来の値を定数へ出した）。
/// tako-app の `drive_prompt_flows` がこの秒数で打ち切る
pub const FLOW_TIMEOUT_SECS: u32 = 120;

/// 予算の不変条件をコンパイル時に固定する（#1259）。
/// - 素の `try_peer`（書き込み 5 秒 + 受信確認 6 秒）が収まる
/// - 諦めた後にキー操作経路が完走できる余地がフロー全体に残る
const _: () = assert!(PEER_BUDGET_SECS >= 12);
const _: () = assert!(PEER_BUDGET_SECS * 2 < FLOW_TIMEOUT_SECS);

/// 起動コマンド待ちで送達フローを保留できる合計時間（秒）。
///
/// 旧実装は保留のあいだ `created_at` を毎 tick 現在時刻へ戻していたので、
/// 同じペインへ起動コマンドが積まれ続けるかぎり**総合タイムアウトに一生届かず**、
/// しかも痕跡も残らなかった。保留は正しい（素のシェルへ本文を貼らないため = #640）が、
/// 無期限に無音で待つのは正しくない
pub const HOLD_BUDGET_SECS: u32 = 180;

/// 待ちを打ち切るかの**方針**（純粋関数。上限だけを見る）。
///
/// 実際の打ち切りは [`peer_wait_now`] を使う（段階の判定は CAS でないと
/// 二重投函を防げない）。この関数は方針の単体テスト用に残してある
pub fn peer_wait(phase: PeerPhase, elapsed_secs: u32) -> PeerWait {
    if legacy_silent() || elapsed_secs < PEER_BUDGET_SECS {
        return PeerWait::KeepWaiting;
    }
    match phase {
        PeerPhase::Resolving => PeerWait::FallbackToKeys,
        PeerPhase::Sending | PeerPhase::Verifying => PeerWait::StopUnconfirmed,
    }
}

/// peer の背景試行を打ち切るかを**実際に決める**（#1259）。
///
/// 上限を過ぎていたら [`PeerAttemptState::cancel_before_send`] の CAS で
/// 「背景スレッドより先に諦められたか」を確かめる。勝てた = まだ 1 バイトも
/// 送っていないのでキー経路へ落ちてよい。負けた = 書き込みが始まっているので
/// 落ちずに未確認で決着させる（落ちると二重投函）
pub fn peer_wait_now(state: &PeerAttemptState, elapsed_secs: u32) -> PeerWait {
    if legacy_silent() || elapsed_secs < PEER_BUDGET_SECS {
        return PeerWait::KeepWaiting;
    }
    if state.cancel_before_send() {
        PeerWait::FallbackToKeys
    } else {
        PeerWait::StopUnconfirmed
    }
}

/// 決着した送達を応答へ出し続ける時間（秒）。
///
/// 成功は「送った直後に確かめる」ためだけに要るので短命にする（出し続けると
/// 過去の送達が `tako_read_pane` の応答に残り続けて異常が埋もれる）。
/// 打ち切りは次の送達まで残す = master が後から「なぜ届かなかったか」を問える
pub const DELIVERED_VISIBLE_SECS: u32 = 300;

/// この顛末をまだ応答へ出すか（`age_secs` = 決着してからの経過）。
///
/// 「まだ出すか」であって「まだ回っているか」ではない。後者は [`State::is_pending`]
pub fn still_visible(status: &Status, age_secs: u32) -> bool {
    match status.state {
        State::Delivered => age_secs < DELIVERED_VISIBLE_SECS,
        // 待ち・打ち切りは消さない（無音に戻したら #1259 の再来）
        State::Queued | State::Waiting | State::GaveUp => true,
    }
}

/// 積む**前**のそのペインの送達状態のうち、「この送達が後ろに並ぶ」対象だけを通す（#1292）。
///
/// `tako_send_input(await_prompt=true)` は `queued` を即返すが、先行フローがまだ
/// 回っているならその状態を載せる（#1259: いま何を待っているかを応答から読める）。
/// ところが `prompt_delivery_states` は**ペインを閉じたときにしか消えない**ので、
/// 素通しすると新しい送達の応答が**前回の決着済みの顛末**（`delivered` / `gave_up`）を
/// 名乗る。1 時間前の `gave_up` は「積んだ瞬間に失敗した」と読め、10 秒前の
/// `delivered` に至っては master が届いたと判断して監視をやめる —— そこから
/// [`FLOW_TIMEOUT_SECS`] 秒の送達フローが始まるところなのに。
///
/// 判定は `state` の語彙だけを見る（`Status` へ戻さない = 応答 JSON をそのまま運ぶ）。
/// 読めない綴りは通さない: 新しい送達は実際に `queued` なので、そう答えるほうが嘘にならない
pub fn pending_predecessor(status: Option<serde_json::Value>) -> Option<serde_json::Value> {
    let status = status?;
    if legacy_passthrough() {
        return Some(status);
    }
    let state = State::parse(status.get("state")?.as_str()?)?;
    state.is_pending().then_some(status)
}

/// `TAKO_1292_LEGACY=1` で **#1292 前**の挙動（決着済みの顛末も素通し）へ戻す。
/// 同一バイナリで A/B を取る入口
pub fn legacy_passthrough() -> bool {
    std::env::var_os("TAKO_1292_LEGACY").is_some()
}

/// `TAKO_1259_LEGACY=1` で **#1259 前**の挙動へ戻す。
///
/// 戻るのは 2 点: ①顛末を `persist.log` へ書かない（`eprintln!` だけ = 無音）
/// ②peer の背景試行を上限なしで待つ。同一バイナリで A/B を取る入口
pub fn legacy_silent() -> bool {
    std::env::var_os("TAKO_1259_LEGACY").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 理由コードは重複しない() {
        let all = [
            Stall::SessionGone,
            Stall::HoldForCommandFlow,
            Stall::BehindFlow,
            Stall::TuiNotReady,
            Stall::NoInputBox,
            Stall::ChoiceDialog,
            Stall::TrustDialog,
            Stall::PeerPending,
            Stall::PeerSendStalled,
            Stall::QueueDrain,
            Stall::PasteNotReflected,
            Stall::EnterResend,
        ];
        let mut codes: Vec<&str> = all.iter().map(|s| s.code()).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(before, codes.len(), "理由コードが重複している: {codes:?}");
        for s in all {
            assert!(!s.note().ja().is_empty(), "{} の ja が空", s.code());
            assert!(!s.note().en().is_empty(), "{} の en が空", s.code());
        }
    }

    #[test]
    fn 待ちのログ行はペイン番号と理由を持つ() {
        let status = Status::waiting(Stall::PeerPending, 42);
        let line = status.log_line(Some(1627), None);
        assert!(line.contains("pane=1627"), "{line}");
        assert!(line.contains("状態=waiting"), "{line}");
        assert!(line.contains("理由=peer_pending"), "{line}");
        assert!(line.contains("経過=42s"), "{line}");
    }

    /// #1259 の症状は「打ち切ったのに何も残らない」。打ち切りの行には
    /// **顛末と、諦めた瞬間に待っていたもの**の両方が要る
    #[test]
    fn 打ち切りのログ行は顛末と待っていた理由を持つ() {
        let status = Status::gave_up("flow_timeout", Some(Stall::NoInputBox), 121);
        let line = status.log_line(Some(1627), None);
        assert!(line.contains("状態=gave_up"), "{line}");
        assert!(line.contains("顛末=flow_timeout"), "{line}");
        assert!(line.contains("理由=no_input_box"), "{line}");
    }

    #[test]
    fn 送達のログ行は経路を持つ() {
        let line = Status::delivered("peer", "delivered", 3).log_line(Some(9), None);
        assert!(line.contains("状態=delivered"), "{line}");
        assert!(line.contains("経路=peer"), "{line}");
    }

    /// ペイン番号を持たない経路（tmux セッション送達）はセッション名で突き合わせる
    #[test]
    fn ペイン番号が無い経路はセッション名で書く() {
        let line = Status::gave_up("failed", None, 3).log_line(None, Some("tako-abc123"));
        assert!(line.contains("pane=-"), "{line}");
        assert!(line.contains("session=tako-abc123"), "{line}");
    }

    #[test]
    fn 応答jsonは理由コードと注記と経過を持つ() {
        let v = Status::waiting(Stall::ChoiceDialog, 7).to_json_in(Lang::Ja);
        assert_eq!(v["state"], "waiting");
        assert_eq!(v["reason"], "choice_dialog");
        assert_eq!(v["transient"], false);
        assert_eq!(v["elapsed_secs"], 7);
        assert!(
            v["note"].as_str().is_some_and(|s| s.contains("respond")),
            "次の一手が注記に出る: {v}"
        );
        let en = Status::waiting(Stall::ChoiceDialog, 7).to_json_in(Lang::En);
        assert!(en["note"].as_str().is_some_and(|s| s.contains("respond")));
    }

    #[test]
    fn queuedの応答は理由を持たない() {
        let v = Status::queued().to_json_in(Lang::Ja);
        assert_eq!(v["state"], "queued");
        assert!(v["reason"].is_null());
        assert!(v["outcome"].is_null());
    }

    /// 500ms tick で毎回書くと埋まるので、状態が変わったときと心拍だけにする
    #[test]
    fn journalは状態が変わったときと心拍だけ書く() {
        let mut j = Journal::default();
        assert!(
            j.should_log(&Status::waiting(Stall::TuiNotReady, 0)),
            "初回"
        );
        assert!(
            !j.should_log(&Status::waiting(Stall::TuiNotReady, 1)),
            "同じ理由・心拍前は書かない"
        );
        assert!(
            j.should_log(&Status::waiting(Stall::NoInputBox, 2)),
            "理由が変わったら書く"
        );
        assert!(
            !j.should_log(&Status::waiting(Stall::NoInputBox, 3)),
            "同じ理由・心拍前は書かない"
        );
        assert!(
            j.should_log(&Status::waiting(
                Stall::NoInputBox,
                2 + Journal::HEARTBEAT_SECS
            )),
            "心拍を過ぎたら同じ理由でも言い直す"
        );
    }

    /// 打ち切りは待ちと同じ理由でも**必ず**書く（顛末が変わる = キーが変わる）
    #[test]
    fn 打ち切りは同じ理由でも書く() {
        let mut j = Journal::default();
        assert!(j.should_log(&Status::waiting(Stall::PeerPending, 0)));
        assert!(
            j.should_log(&Status::gave_up(
                "peer_timeout",
                Some(Stall::PeerPending),
                1
            )),
            "待ち → 打ち切りは状態が変わっているので書く"
        );
    }

    /// #790 の不変条件: 送った後は落ちない（落ちると二重投函）
    #[test]
    fn peerの上限は送る前だけキー経路へ落ちる() {
        assert_eq!(
            peer_wait(PeerPhase::Resolving, PEER_BUDGET_SECS),
            PeerWait::FallbackToKeys
        );
        assert_eq!(
            peer_wait(PeerPhase::Sending, PEER_BUDGET_SECS),
            PeerWait::StopUnconfirmed
        );
        assert_eq!(
            peer_wait(PeerPhase::Verifying, PEER_BUDGET_SECS),
            PeerWait::StopUnconfirmed
        );
    }

    /// CAS の要点: 諦めと書き込みは**どちらか一方だけ**が勝つ
    #[test]
    fn 諦めと書き込みはどちらか一方だけが勝つ() {
        // 先に諦めた → 背景スレッドは書き込めない
        let a = PeerAttemptState::new();
        assert!(a.cancel_before_send(), "まだ resolving なので諦められる");
        assert!(!a.begin_sending(), "取り消し済みなので書き込んではならない");
        assert_eq!(a.as_str(), "cancelled");

        // 先に書き始めた → UI は諦められない（落ちると二重投函）
        let b = PeerAttemptState::new();
        assert!(b.begin_sending());
        assert!(!b.cancel_before_send(), "送り始めた後は落ちてはならない");
        assert_eq!(b.as_str(), "sending");
        b.begin_verifying();
        assert_eq!(b.as_str(), "verifying");
        assert!(!b.cancel_before_send());

        // 二重の諦め・二重の送信開始も 1 回だけ勝つ
        let c = PeerAttemptState::new();
        assert!(c.cancel_before_send());
        assert!(!c.cancel_before_send());
        let d = PeerAttemptState::new();
        assert!(d.begin_sending());
        assert!(!d.begin_sending());
    }

    /// `peer_wait_now` は上限内では CAS を撃たない（撃つと待っている試行を殺す）
    #[test]
    fn 上限内はcasを撃たない() {
        let st = PeerAttemptState::new();
        assert_eq!(
            peer_wait_now(&st, PEER_BUDGET_SECS - 1),
            PeerWait::KeepWaiting
        );
        assert_eq!(st.as_str(), "resolving", "上限内で取り消してはいけない");
        assert_eq!(
            peer_wait_now(&st, PEER_BUDGET_SECS),
            PeerWait::FallbackToKeys
        );
        assert_eq!(st.as_str(), "cancelled");
    }

    #[test]
    fn peerの上限内はどの段階でも待つ() {
        for phase in [
            PeerPhase::Resolving,
            PeerPhase::Sending,
            PeerPhase::Verifying,
        ] {
            assert_eq!(
                peer_wait(phase, PEER_BUDGET_SECS - 1),
                PeerWait::KeepWaiting,
                "{phase:?}"
            );
        }
    }

    /// 打ち切りは消えない（無音に戻したら #1259 の再来）。成功だけ短命
    #[test]
    fn 打ち切りは残り続け成功は短命() {
        let gave_up = Status::gave_up("flow_timeout", Some(Stall::NoInputBox), 120);
        assert!(still_visible(&gave_up, DELIVERED_VISIBLE_SECS * 10));
        let delivered = Status::delivered("peer", "delivered", 3);
        assert!(still_visible(&delivered, DELIVERED_VISIBLE_SECS - 1));
        assert!(!still_visible(&delivered, DELIVERED_VISIBLE_SECS));
        assert!(still_visible(
            &Status::waiting(Stall::PeerPending, 9),
            99_999
        ));
    }

    /// #1294: 「送ったかもしれない」は未達と別の値になる。
    /// 送達フロー側の宣言（`PeerSendStalled` = 再送してはいけない）と
    /// レジストリの記録が食い違わないための表
    #[test]
    fn 書き込み後に確認が取れない顛末は未達と断定しない() {
        for code in [Stall::PeerSendStalled.code(), PEER_UNCONFIRMED] {
            assert_eq!(
                outcome_confidence(code),
                Confidence::Unverified,
                "{code} は送った可能性があるので未達と断定できない"
            );
            assert!(
                !outcome_confidence(code).may_auto_resend(),
                "{code} で自動再送を撃つと二重投函になる"
            );
        }
    }

    /// 未達と断定できる顛末は従来どおり（回帰なし）。
    /// **既定が未達側**であること（知らないコードは救済側へ倒さない）も固定する
    #[test]
    fn 未達と断定できる顛末は自動再送の対象のまま() {
        for code in [
            "paste_not_reflected",
            "residual_after_retries",
            "choice_dialog",
            "trust_dialog_blocked",
            "flow_timeout",
            "hold_timeout",
            "peer_refused",
            "未知の顛末コード",
        ] {
            assert_eq!(
                outcome_confidence(code),
                Confidence::Undelivered,
                "{code} は未達確定のまま"
            );
            assert!(outcome_confidence(code).may_auto_resend(), "{code}");
        }
    }

    #[test]
    fn 送達できた顛末は届いた側() {
        for code in [VERIFIED, DELIVERED] {
            assert_eq!(outcome_confidence(code), Confidence::Delivered, "{code}");
            assert!(
                !outcome_confidence(code).may_auto_resend(),
                "{code} は再送そのものが要らない"
            );
        }
    }

    /// `Stall` の宣言（再送してはいけない）と表が対応している。
    /// 停滞理由に「書いた後」の系統を足したのに表を直し忘れたら落とす
    #[test]
    fn 再送禁止の停滞理由は表でも未確認側にある() {
        // `PeerSendStalled` の note は「再送しない」と明言している（宣言の正本）
        assert!(
            Stall::PeerSendStalled.note().ja().contains("再送しない"),
            "宣言が変わったら表も見直すこと"
        );
        assert_eq!(
            outcome_confidence(Stall::PeerSendStalled.code()),
            Confidence::Unverified
        );
        // 「まだ 1 バイトも送っていない」段階の理由は未達側でよい
        assert_eq!(
            outcome_confidence(Stall::NoInputBox.code()),
            Confidence::Undelivered
        );
    }

    /// #1292: 「まだ出すか」（`still_visible`）と「まだ回っているか」（`is_pending`）は別の問い。
    /// 決着した顛末は `tako_read_pane` へ出し続けるが、後ろに並ぶ相手にはならない
    #[test]
    fn 未決着と表示寿命は別の問い() {
        assert!(State::Queued.is_pending());
        assert!(State::Waiting.is_pending());
        assert!(!State::Delivered.is_pending());
        assert!(!State::GaveUp.is_pending());
        // 打ち切りは無期限に「出す」が、後ろに並ぶ相手ではない
        let gave_up = Status::gave_up("flow_timeout", Some(Stall::NoInputBox), 120);
        assert!(still_visible(&gave_up, DELIVERED_VISIBLE_SECS * 10));
        assert!(!gave_up.state.is_pending());
    }

    /// 綴りは `as_str` の 1 実装から来る（応答 JSON と語彙が食い違わない）
    #[test]
    fn 状態の綴りは往復する() {
        for state in [
            State::Queued,
            State::Waiting,
            State::Delivered,
            State::GaveUp,
        ] {
            assert_eq!(State::parse(state.as_str()), Some(state), "{state:?}");
        }
        assert_eq!(State::parse("unknown_state"), None);
        assert_eq!(State::parse(""), None);
    }

    /// #1292: 積む前の状態のうち後ろに並ぶ対象だけを通す。
    /// 決着済み・読めない綴り・状態なしはどれも `None`（呼び出し側が `queued` を出す）
    #[test]
    fn 後ろに並ぶ対象は未決着だけ() {
        let waiting = Status::waiting(Stall::PeerPending, 42).to_json();
        assert_eq!(
            pending_predecessor(Some(waiting.clone())),
            Some(waiting),
            "未決着はそのまま通す（#1259 の挙動は据え置き）"
        );
        assert_eq!(
            pending_predecessor(Some(Status::queued().to_json())),
            Some(Status::queued().to_json())
        );
        for settled in [
            Status::delivered("peer", "delivered", 3).to_json(),
            Status::gave_up("flow_timeout", Some(Stall::NoInputBox), 120).to_json(),
        ] {
            assert_eq!(
                pending_predecessor(Some(settled.clone())),
                None,
                "決着済みを後ろに並ぶ相手として通している: {settled}"
            );
        }
        assert_eq!(pending_predecessor(None), None);
        assert_eq!(
            pending_predecessor(Some(serde_json::json!({ "state": "unknown_state" }))),
            None,
            "読めない綴りは通さない（新しい送達は実際に queued なので嘘にならない）"
        );
        assert_eq!(pending_predecessor(Some(serde_json::json!({}))), None);
    }
}

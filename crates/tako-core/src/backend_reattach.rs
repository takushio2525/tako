//! backend_reattach — attach クライアントだけが外から終わったときの再 attach（Issue #1857）
//!
//! tmux バックエンド（FR-5）のペインは「tako の PTY → tmux の attach クライアント →
//! tmux サーバー上のセッション → シェル / claude」の 3 段で、tako が子プロセスとして
//! 見ているのは **attach クライアント**である。クライアントが終わる理由は 2 種類あり、
//! 見分けないと事故になる:
//!
//! - セッションそのものが終わった（シェルの exit・`kill-session`）→ ペインを閉じてよい
//! - クライアントだけが終わった（外からの SIGTERM / SIGHUP / SIGKILL）
//!   → セッションと中のプロセスは生きている
//!
//! 2026-09-30 の実例: BSD pkill はパターンより後ろのオプションを解釈しないため、
//! `pkill -f '<パターン>' -u <uid>` の `-u` までパターン扱いになり、`tmux -u -L tako …` で
//! 動いていた attach クライアントが全部 SIGTERM された。tmux サーバー・24 セッション・
//! 中の claude はすべて生きていたのに、tako は前者と区別せずペイン 24 枚とタブを閉じた。
//!
//! ここは判断だけを持つ（純粋関数と、器への 1 回の問い合わせ）。時計・端末の張り替え・
//! 表示は呼び出し側（tako-app）が持つ（[`crate::ssh_reconnect`] と同じ構え）。
//!
//! # 判断の順序（[`ReattachState::decide`]）
//!
//! 1. セッションが無い / サーバーごと無い → 従来どおり閉じる
//!    （サーバーごと落ちた場合の扱いは #1197 の領分なので変えない）
//! 2. 別のクライアントが attach 中 → 閉じる。多重起動した後発が `new-session -A -D` で
//!    切り離した形で、再 attach すると奪い合いになる（FR-5.10 の復元強奪ガードと同じ考え）
//! 3. 直近 [`WINDOW`] の再 attach が [`MAX_ATTEMPTS`] 回に達している → 閉じずに止まる
//! 4. それ以外（生きている / 確かめられなかった）→ 再 attach
//!
//! 「確かめられなかった」（問い合わせの打ち切り・想定外のエラー）を「無い」へ倒さない（#1597）。
//! 再 attach は attach 専用のコマンド（[`crate::tmux_backend::reattach_options`]）なので、
//! 本当に無ければ新しいセッションを作らずに失敗し、もう一度ここへ戻ってきて 1. で閉じる。

use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::i18n::Lang;

/// 直近 [`WINDOW`] の間に撃ってよい再 attach の回数。
///
/// 外から殺し続ける何か（ループする pkill・監視スクリプトの誤爆）が居ると、再 attach は
/// 殺され直すだけになる。数えずに撃つと tmux のクライアント起動を永久に回し続けるので頭を置く
pub const MAX_ATTEMPTS: u32 = 5;

/// 再 attach の回数を数える窓。長く繋がっていたペインが久しぶりに殺されたときは
/// 数え直しになる（窓から外れた過去の回は数えない）
pub const WINDOW: Duration = Duration::from_secs(60);

/// 器への問い合わせ（`list-clients`）の上限（#1503: 外部コマンドは上限つきで待つ）。
/// 普段は数 ms で返るので、3 秒は「器が固まっている」ときにだけ効く
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// `attempt` 回目（1 始まり）の再 attach を撃つまでの待ち。
///
/// 1 回目も 0 にしない: 一斉に殺した側（pkill 等）がまだ走っている間に撃つと、
/// 立てた直後のクライアントまで殺され直す。後ろは倍々で寝かせ、4 秒で頭打ちにする
pub fn backoff(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(4);
    Duration::from_millis((250u64 << shift).min(4_000))
}

/// 器に聞いたセッションの生死（3 値 + サーバー不在。#1597）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionProbe {
    /// セッションは生きている。`clients` は attach 中のクライアントの pid
    /// （終わったばかりの自分のクライアントがまだ載っていることがある）
    Alive { clients: Vec<u32> },
    /// セッションが無い（シェルの exit・`kill-session`）
    SessionGone,
    /// サーバーごと無い（#1197 の領分。従来どおり閉じる）
    ServerGone,
    /// 材料が採れなかった（打ち切り・起動失敗・想定外のエラー）。「無い」ではない
    Unknown,
}

/// `tmux list-clients -t =<session> -F '#{client_pid}'` の結果を読む（純粋関数）。
///
/// エラー文言は tmux 3.6b の実測（ロケールに依らず英語）:
///
/// - `can't find session: <名>` = セッションが無い
/// - `no server running on <ソケット>` = サーバーが居ない
/// - `error connecting to <ソケット> (No such file or directory)` = ソケットごと無い
/// - `error connecting to <ソケット> (Connection refused)` = 死んだサーバーのソケットが残っている
pub fn classify_probe(success: bool, stdout: &str, stderr: &str) -> SessionProbe {
    if success {
        let clients = stdout
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .filter(|pid| *pid != 0)
            .collect();
        return SessionProbe::Alive { clients };
    }
    let err = stderr.trim();
    if err.starts_with("can't find session") {
        SessionProbe::SessionGone
    } else if err.starts_with("no server running")
        || (err.starts_with("error connecting to")
            && (err.contains("No such file or directory") || err.contains("Connection refused")))
    {
        SessionProbe::ServerGone
    } else {
        SessionProbe::Unknown
    }
}

/// 器へ 1 回だけ問い合わせて、セッションの生死と attach 中のクライアントを返す。
///
/// 待ちは [`crate::probe::wait_with_timeout`] の 1 実装を通す（上限 [`PROBE_TIMEOUT`]）。
/// 起動できない・打ち切った・想定外のエラーはすべて [`SessionProbe::Unknown`]
pub fn probe_session(socket: &str, session: &str) -> SessionProbe {
    let target = crate::tmux::exact_target(session);
    let mut command = crate::tmux::tmux_command(Some(socket));
    command
        .args(["list-clients", "-t", &target, "-F", "#{client_pid}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Ok(child) = command.spawn() else {
        return SessionProbe::Unknown;
    };
    match crate::probe::wait_with_timeout(child, "tmux list-clients", PROBE_TIMEOUT) {
        crate::probe::Outcome::Done {
            status,
            stdout,
            stderr,
        } => classify_probe(
            status.success(),
            &String::from_utf8_lossy(&stdout),
            &String::from_utf8_lossy(&stderr),
        ),
        _ => SessionProbe::Unknown,
    }
}

/// attach クライアントの終わり方（終了ステータスだけから決める。**画面は読まない**）。
///
/// tmux のクライアントは SIGTERM / SIGHUP を受けると後始末をして **exit 1** で終わる
/// （画面に `[terminated]` / `[lost tty]` を出す）。SIGKILL はそのまま signal 9
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClientExit {
    Code(i32),
    Signal(i32),
    #[default]
    Unknown,
}

impl ClientExit {
    pub fn from_status(status: Option<std::process::ExitStatus>) -> Self {
        let Some(status) = status else {
            return ClientExit::Unknown;
        };
        if let Some(code) = status.code() {
            return ClientExit::Code(code);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return ClientExit::Signal(signal);
            }
        }
        ClientExit::Unknown
    }

    /// persist.log と応答の `last_reason` に載せる短い名前
    pub fn label(self) -> String {
        match self {
            ClientExit::Code(code) => format!("exit {code}"),
            ClientExit::Signal(signal) => match signal_name(signal) {
                Some(name) => format!("signal {signal} ({name})"),
                None => format!("signal {signal}"),
            },
            ClientExit::Unknown => "unknown".to_string(),
        }
    }
}

fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        9 => "SIGKILL",
        15 => "SIGTERM",
        _ => return None,
    })
}

/// ペインを閉じる理由（[`Verdict::Close`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCause {
    SessionGone,
    ServerGone,
    /// 別のクライアントが attach 中（多重起動の後発が `-D` で切り離した）
    TakenOver {
        clients: usize,
    },
}

/// attach クライアントが終わったときにすること
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 従来どおりペインを閉じる（セッションは kill しない = FR-5 / #30）
    Close(CloseCause),
    /// `delay` 待ってから再 attach する（これが直近の窓で `attempt` 回目）。
    /// `unverified` = 生死を確かめられないまま撃つ（attach 専用なので、無ければ失敗して戻る）
    Reattach {
        attempt: u32,
        delay: Duration,
        unverified: bool,
    },
    /// 上限に達した。**閉じずに**「再接続できない」を出して止まる
    GiveUp { attempts: u32 },
}

/// 再 attach の進み具合（応答の `status`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 器へ生死を問い合わせている
    Checking,
    /// 再 attach を撃つまで待っている / 撃った直後
    Waiting,
    /// 再 attach した（クライアントは動いている）
    Attached,
    /// 上限に達して止まっている（ペインは残る）
    GaveUp,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Checking => "checking",
            Status::Waiting => "waiting",
            Status::Attached => "attached",
            Status::GaveUp => "gave_up",
        }
    }
}

/// ペイン 1 枚ぶんの再 attach の記録。
///
/// 一度でもクライアントが外から終わったペインにだけ作り、ペインを閉じるまで持つ
/// （再 attach した後も回数と直近の理由を CLI / MCP から読めるようにするため）
#[derive(Debug, Clone)]
pub struct ReattachState {
    /// 直近 [`WINDOW`] の間に撃った再 attach の時刻
    recent: Vec<Instant>,
    /// これまでに撃った再 attach の累計（手動を含む）
    total: u32,
    status: Status,
    last_exit: ClientExit,
    /// 直近にクライアントが終わった時刻（UNIX 秒）
    last_at: u64,
    /// いま扱っている端末（`TerminalSession::serial`）。同じ PTY から Exit と ChildExit の
    /// 2 通が届く（terminal.rs で両方 `SessionNotice::Exited` になる）ので 1 回に畳む
    handling: Option<u64>,
}

impl Default for ReattachState {
    fn default() -> Self {
        Self::new()
    }
}

impl ReattachState {
    pub fn new() -> Self {
        Self {
            recent: Vec::new(),
            total: 0,
            status: Status::Checking,
            last_exit: ClientExit::Unknown,
            last_at: 0,
            handling: None,
        }
    }

    /// 端末 `serial` の終わりを受け付ける。**同じ端末の 2 通目は false**（何もしない）
    pub fn begin_exit(&mut self, serial: u64, exit: ClientExit, now_unix: u64) -> bool {
        if self.handling == Some(serial) {
            return false;
        }
        self.handling = Some(serial);
        self.status = Status::Checking;
        self.last_exit = exit;
        self.last_at = now_unix;
        true
    }

    /// 器の答え（`probe`）から次にすることを決める。
    ///
    /// `own_client` は終わったばかりの自分のクライアントの pid。SIGKILL のように
    /// サーバーへ別れを告げずに死んだクライアントは、サーバーが気づくまで
    /// `list-clients` に残るので、それを「別のクライアント」と数えない
    pub fn decide(
        &mut self,
        probe: &SessionProbe,
        own_client: Option<u32>,
        now: Instant,
    ) -> Verdict {
        let unverified = match probe {
            SessionProbe::SessionGone => return Verdict::Close(CloseCause::SessionGone),
            SessionProbe::ServerGone => return Verdict::Close(CloseCause::ServerGone),
            SessionProbe::Alive { clients } => {
                let others = clients
                    .iter()
                    .filter(|pid| Some(**pid) != own_client)
                    .count();
                if others > 0 {
                    return Verdict::Close(CloseCause::TakenOver { clients: others });
                }
                false
            }
            SessionProbe::Unknown => true,
        };
        self.recent
            .retain(|at| now.saturating_duration_since(*at) < WINDOW);
        let done = self.recent.len() as u32;
        if done >= MAX_ATTEMPTS {
            self.status = Status::GaveUp;
            return Verdict::GiveUp { attempts: done };
        }
        self.recent.push(now);
        self.total += 1;
        self.status = Status::Waiting;
        let attempt = done + 1;
        Verdict::Reattach {
            attempt,
            delay: backoff(attempt),
            unverified,
        }
    }

    /// 再 attach のクライアントを立てられた
    pub fn mark_attached(&mut self) {
        self.status = Status::Attached;
    }

    /// 再 attach のクライアントを立てられなかった（PTY の起動失敗）。閉じずに止まる
    pub fn mark_gave_up(&mut self) {
        self.status = Status::GaveUp;
    }

    /// 手動の再接続（`tako persist reattach`）。上限を数え直し、1 回ぶんとして数える
    pub fn begin_manual(&mut self, now: Instant) {
        self.recent.clear();
        self.recent.push(now);
        self.total += 1;
        self.handling = None;
        self.status = Status::Waiting;
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// 直近 [`WINDOW`] の間に撃った再 attach の回数
    pub fn attempts(&self) -> usize {
        self.recent.len()
    }

    pub fn total(&self) -> u32 {
        self.total
    }

    pub fn last_exit(&self) -> ClientExit {
        self.last_exit
    }

    /// 応答（`tako list` / `tako read` の `backend_reattach`）に載せる形。
    /// `pane` は止まったときの次の一手（コマンド）に入れる
    pub fn to_json(&self, pane: u64, lang: Lang) -> serde_json::Value {
        let mut v = serde_json::json!({
            "status": self.status.as_str(),
            "attempts": self.attempts(),
            "total": self.total,
            "max_attempts": MAX_ATTEMPTS,
            "window_secs": WINDOW.as_secs(),
            "last_reason": self.last_exit.label(),
            "last_at": self.last_at,
        });
        if self.status == Status::GaveUp {
            v["next_step"] = serde_json::json!(retry_command(pane));
            v["message"] = serde_json::json!(gave_up_notice(lang, pane));
        }
        v
    }
}

/// 止まったペインを手で再接続するコマンド（#322: 最簡形。別のペインから打つので `--pane` は要る）
pub fn retry_command(pane: u64) -> String {
    format!("tako persist reattach --pane {pane}")
}

/// persist.log へ書く 1 行（FR-5.15 と同じ規約: ペイン ID・セッション名・理由だけ。
/// **ペインの内容・送信テキスト・トークンは載せない**）。
///
/// セッションごと終わった普通の終わり方（シェルの exit）は `None` = 書かない。
/// 1 日に何百回も起きるので、書くと他の記録が埋もれる
pub fn log_line(pane: u64, session: &str, exit: ClientExit, verdict: &Verdict) -> Option<String> {
    let tail = match verdict {
        Verdict::Close(CloseCause::SessionGone | CloseCause::ServerGone) => return None,
        Verdict::Close(CloseCause::TakenOver { clients }) => {
            format!("別のクライアント {clients} 本が attach 中なので閉じる（セッションは残す）")
        }
        Verdict::Reattach {
            attempt,
            unverified: false,
            ..
        } => format!("再 attach（{attempt} 回目）"),
        Verdict::Reattach {
            attempt,
            unverified: true,
            ..
        } => format!("再 attach（{attempt} 回目。セッションの生死は確かめられなかった）"),
        Verdict::GiveUp { attempts } => format!(
            "再 attach の上限（{} 秒に {attempts} 回）に達したので止める（ペインは閉じない）",
            WINDOW.as_secs()
        ),
    };
    Some(format!(
        "attach クライアント異常終了: pane={pane} session={session} 理由={} → {tail}",
        exit.label()
    ))
}

/// 手動の再接続を persist.log へ残す 1 行
pub fn manual_log_line(pane: u64, session: &str) -> String {
    format!("attach クライアントの手動再 attach: pane={pane} session={session}")
}

/// ペインのヘッダに出す一言（`None` = 出さない）
pub fn chip_label(lang: Lang, status: Status, attempts: usize) -> Option<String> {
    match (status, lang) {
        (Status::Attached, _) => None,
        (Status::Checking, Lang::Ja) => Some("tmux へ再接続しています…".to_string()),
        (Status::Checking, Lang::En) => Some("Reconnecting to tmux…".to_string()),
        (Status::Waiting, Lang::Ja) => Some(format!(
            "tmux へ再接続しています…（{attempts}/{MAX_ATTEMPTS}）"
        )),
        (Status::Waiting, Lang::En) => {
            Some(format!("Reconnecting to tmux… ({attempts}/{MAX_ATTEMPTS})"))
        }
        (Status::GaveUp, Lang::Ja) => {
            Some("tmux へ再接続できません（クリックで再試行）".to_string())
        }
        (Status::GaveUp, Lang::En) => Some("Can't reconnect to tmux (click to retry)".to_string()),
    }
}

/// 止まったときにペインの画面へ出す 2 行（何が起きたか / 次の一手）。
///
/// ヘッダのチップは幅の狭いペインでは出ないので、画面にも残す。コマンドは**行を分けて**出す
/// （説明と続けると長い行の途中で折り返され、`--pane` と番号が別の行に割れる = 実測）
pub fn gave_up_screen_lines(lang: Lang, pane: u64) -> [String; 2] {
    let secs = WINDOW.as_secs();
    let command = retry_command(pane);
    match lang {
        Lang::Ja => [
            format!(
                "[tako] tmux のセッションへ再接続できませんでした（{secs} 秒に {MAX_ATTEMPTS} 回）。\
                 セッションと中のプロセスは残っています"
            ),
            format!("[tako] 再試行: {command}"),
        ],
        Lang::En => [
            format!(
                "[tako] Could not reconnect to the tmux session ({MAX_ATTEMPTS} tries in {secs}s). \
                 The session and its processes are still running"
            ),
            format!("[tako] Retry: {command}"),
        ],
    }
}

/// [`gave_up_screen_lines`] を 1 行に繋いだもの（応答の `message`）
pub fn gave_up_notice(lang: Lang, pane: u64) -> String {
    let [what, next] = gave_up_screen_lines(lang, pane);
    format!("{what}. {}", next.trim_start_matches("[tako] "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn 待ちは250msから倍々で4秒で頭打ち() {
        let got: Vec<u64> = (1..=7).map(|n| backoff(n).as_millis() as u64).collect();
        assert_eq!(got, vec![250, 500, 1000, 2000, 4000, 4000, 4000]);
        // 0 回目（呼ばれない想定）も 1 回目と同じ扱いで、桁あふれしない
        assert_eq!(backoff(0), Duration::from_millis(250));
        assert_eq!(backoff(u32::MAX), Duration::from_millis(4000));
    }

    #[test]
    fn 問い合わせの結果を3値とサーバー不在へ読み分ける() {
        assert_eq!(
            classify_probe(true, "", ""),
            SessionProbe::Alive { clients: vec![] }
        );
        assert_eq!(
            classify_probe(true, "123\n 456 \n\n0\nabc\n", ""),
            SessionProbe::Alive {
                clients: vec![123, 456]
            }
        );
        assert_eq!(
            classify_probe(false, "", "can't find session: tako-x\n"),
            SessionProbe::SessionGone
        );
        assert_eq!(
            classify_probe(false, "", "no server running on /private/tmp/tmux-501/tako"),
            SessionProbe::ServerGone
        );
        assert_eq!(
            classify_probe(
                false,
                "",
                "error connecting to /private/tmp/tmux-501/x (No such file or directory)"
            ),
            SessionProbe::ServerGone
        );
        assert_eq!(
            classify_probe(
                false,
                "",
                "error connecting to /private/tmp/tmux-501/x (Connection refused)"
            ),
            SessionProbe::ServerGone
        );
        // 想定外のエラー・空の失敗は「無い」ではない（#1597）
        assert_eq!(
            classify_probe(false, "", "error connecting to /x (Permission denied)"),
            SessionProbe::Unknown
        );
        assert_eq!(classify_probe(false, "", ""), SessionProbe::Unknown);
    }

    #[test]
    fn セッションが無いかサーバーごと無ければ従来どおり閉じる() {
        let now = Instant::now();
        let mut st = ReattachState::new();
        assert_eq!(
            st.decide(&SessionProbe::SessionGone, Some(10), now),
            Verdict::Close(CloseCause::SessionGone)
        );
        assert_eq!(
            st.decide(&SessionProbe::ServerGone, Some(10), now),
            Verdict::Close(CloseCause::ServerGone)
        );
        // 閉じる判断は回数を消費しない
        assert_eq!(st.total(), 0);
    }

    #[test]
    fn 生きていて誰もattachしていなければ再attachする() {
        let now = Instant::now();
        let mut st = ReattachState::new();
        let v = st.decide(&SessionProbe::Alive { clients: vec![] }, Some(10), now);
        assert_eq!(
            v,
            Verdict::Reattach {
                attempt: 1,
                delay: backoff(1),
                unverified: false
            }
        );
        assert_eq!(st.status(), Status::Waiting);
        st.mark_attached();
        assert_eq!(st.status(), Status::Attached);
    }

    #[test]
    fn 死んだ自分のクライアントがまだ一覧に残っていても別のクライアントと数えない() {
        let now = Instant::now();
        let mut st = ReattachState::new();
        let v = st.decide(&SessionProbe::Alive { clients: vec![10] }, Some(10), now);
        assert!(matches!(v, Verdict::Reattach { attempt: 1, .. }), "{v:?}");
    }

    #[test]
    fn 別のクライアントがattach中なら奪わずに閉じる() {
        let now = Instant::now();
        let mut st = ReattachState::new();
        let v = st.decide(
            &SessionProbe::Alive {
                clients: vec![10, 20, 30],
            },
            Some(10),
            now,
        );
        assert_eq!(v, Verdict::Close(CloseCause::TakenOver { clients: 2 }));
        // 自分の pid が分からないときは載っている全員を他人と数える（奪わない側へ倒す）
        let v = st.decide(&SessionProbe::Alive { clients: vec![10] }, None, now);
        assert_eq!(v, Verdict::Close(CloseCause::TakenOver { clients: 1 }));
    }

    #[test]
    fn 確かめられなかった回は無いと読まずに再attachを撃つ() {
        let now = Instant::now();
        let mut st = ReattachState::new();
        let v = st.decide(&SessionProbe::Unknown, Some(10), now);
        assert_eq!(
            v,
            Verdict::Reattach {
                attempt: 1,
                delay: backoff(1),
                unverified: true
            }
        );
    }

    #[test]
    fn 窓の中で上限に達したら閉じずに止まる() {
        let base = Instant::now();
        let mut st = ReattachState::new();
        let alive = SessionProbe::Alive { clients: vec![] };
        for n in 1..=MAX_ATTEMPTS {
            let v = st.decide(&alive, None, at(base, n as u64));
            assert!(
                matches!(v, Verdict::Reattach { attempt, .. } if attempt == n),
                "{n}: {v:?}"
            );
        }
        let v = st.decide(&alive, None, at(base, 10));
        assert_eq!(
            v,
            Verdict::GiveUp {
                attempts: MAX_ATTEMPTS
            }
        );
        assert_eq!(st.status(), Status::GaveUp);
        // 止まった後も閉じない判断を繰り返す（回数は増えない）
        let v = st.decide(&alive, None, at(base, 11));
        assert!(matches!(v, Verdict::GiveUp { .. }), "{v:?}");
        assert_eq!(st.total(), MAX_ATTEMPTS);
    }

    #[test]
    fn 窓から外れた過去の回は数えない() {
        let base = Instant::now();
        let mut st = ReattachState::new();
        let alive = SessionProbe::Alive { clients: vec![] };
        for n in 0..MAX_ATTEMPTS {
            st.decide(&alive, None, at(base, n as u64));
        }
        // 最後の回から窓の長さ以上あけて殺された = 数え直し
        let later = at(base, MAX_ATTEMPTS as u64 + WINDOW.as_secs());
        let v = st.decide(&alive, None, later);
        assert!(matches!(v, Verdict::Reattach { attempt: 1, .. }), "{v:?}");
        assert_eq!(st.total(), MAX_ATTEMPTS + 1);
    }

    #[test]
    fn 同じ端末のexitとchild_exitは1回に畳む() {
        let mut st = ReattachState::new();
        assert!(st.begin_exit(7, ClientExit::Code(1), 100));
        assert!(!st.begin_exit(7, ClientExit::Unknown, 101));
        // 理由は 1 通目のまま（2 通目の Exit は終了コードを持たない）
        assert_eq!(st.last_exit(), ClientExit::Code(1));
        // 張り替えた後の端末の終わりは別物として受け付ける
        assert!(st.begin_exit(8, ClientExit::Signal(9), 102));
        assert_eq!(st.status(), Status::Checking);
    }

    #[test]
    fn 手動の再接続は上限を数え直す() {
        let base = Instant::now();
        let mut st = ReattachState::new();
        let alive = SessionProbe::Alive { clients: vec![] };
        for n in 0..=MAX_ATTEMPTS {
            st.decide(&alive, None, at(base, n as u64));
        }
        assert_eq!(st.status(), Status::GaveUp);
        st.begin_manual(at(base, 20));
        assert_eq!(st.status(), Status::Waiting);
        assert!(st.begin_exit(9, ClientExit::Code(1), 0));
        let v = st.decide(&alive, None, at(base, 21));
        assert!(matches!(v, Verdict::Reattach { attempt: 2, .. }), "{v:?}");
    }

    #[test]
    fn 終わり方の名前は終了ステータスだけから作る() {
        assert_eq!(ClientExit::Code(1).label(), "exit 1");
        assert_eq!(ClientExit::Signal(9).label(), "signal 9 (SIGKILL)");
        assert_eq!(ClientExit::Signal(15).label(), "signal 15 (SIGTERM)");
        assert_eq!(ClientExit::Signal(31).label(), "signal 31");
        assert_eq!(ClientExit::Unknown.label(), "unknown");
        assert_eq!(ClientExit::from_status(None), ClientExit::Unknown);
    }

    #[cfg(unix)]
    #[test]
    fn 実プロセスの終了ステータスを読み分ける() {
        use std::os::unix::process::ExitStatusExt;
        let exited = std::process::ExitStatus::from_raw(1 << 8);
        assert_eq!(ClientExit::from_status(Some(exited)), ClientExit::Code(1));
        let killed = std::process::ExitStatus::from_raw(9);
        assert_eq!(ClientExit::from_status(Some(killed)), ClientExit::Signal(9));
    }

    #[test]
    fn persistの1行は理由と判断だけで普通の終わり方は書かない() {
        let exit = ClientExit::Code(1);
        let line = log_line(
            3,
            "tako-abc",
            exit,
            &Verdict::Reattach {
                attempt: 2,
                delay: backoff(2),
                unverified: false,
            },
        )
        .expect("再 attach は記録する");
        assert_eq!(
            line,
            "attach クライアント異常終了: pane=3 session=tako-abc 理由=exit 1 → 再 attach（2 回目）"
        );
        let gave_up = log_line(3, "tako-abc", exit, &Verdict::GiveUp { attempts: 5 }).unwrap();
        assert!(gave_up.contains("上限（60 秒に 5 回）"), "{gave_up}");
        assert!(gave_up.contains("ペインは閉じない"), "{gave_up}");
        let taken = log_line(
            3,
            "tako-abc",
            exit,
            &Verdict::Close(CloseCause::TakenOver { clients: 1 }),
        )
        .unwrap();
        assert!(taken.contains("別のクライアント 1 本"), "{taken}");
        assert_eq!(
            log_line(
                3,
                "tako-abc",
                exit,
                &Verdict::Close(CloseCause::SessionGone)
            ),
            None
        );
        assert_eq!(
            log_line(3, "tako-abc", exit, &Verdict::Close(CloseCause::ServerGone)),
            None
        );
    }

    #[test]
    fn 応答は止まったときだけ次の一手を載せる() {
        let base = Instant::now();
        let mut st = ReattachState::new();
        st.begin_exit(1, ClientExit::Code(1), 1_790_000_000);
        st.decide(&SessionProbe::Alive { clients: vec![] }, None, base);
        st.mark_attached();
        let v = st.to_json(4, Lang::Ja);
        assert_eq!(v["status"], "attached");
        assert_eq!(v["attempts"], 1);
        assert_eq!(v["total"], 1);
        assert_eq!(v["max_attempts"], MAX_ATTEMPTS);
        assert_eq!(v["window_secs"], WINDOW.as_secs());
        assert_eq!(v["last_reason"], "exit 1");
        assert_eq!(v["last_at"], 1_790_000_000u64);
        assert!(v.get("next_step").is_none());
        st.mark_gave_up();
        let v = st.to_json(4, Lang::En);
        assert_eq!(v["status"], "gave_up");
        assert_eq!(v["next_step"], "tako persist reattach --pane 4");
        assert!(v["message"].as_str().unwrap().contains("--pane 4"));
    }

    #[test]
    fn 表示の文言は日英とも次の一手を持つ() {
        for lang in [Lang::Ja, Lang::En] {
            let notice = gave_up_notice(lang, 12);
            assert!(
                notice.contains("tako persist reattach --pane 12"),
                "{notice}"
            );
            // 画面ではコマンドを説明と別の行に置く（折り返しでコマンドが割れない）
            let [what, next] = gave_up_screen_lines(lang, 12);
            assert!(!what.contains("tako persist"), "{what}");
            assert!(next.ends_with("tako persist reattach --pane 12"), "{next}");
            assert!(chip_label(lang, Status::Attached, 1).is_none());
            for status in [Status::Checking, Status::Waiting, Status::GaveUp] {
                assert!(chip_label(lang, status, 1).is_some(), "{status:?}");
            }
        }
    }
}

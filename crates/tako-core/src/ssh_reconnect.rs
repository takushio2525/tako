//! SSH ペインの自動再接続（#1040）
//!
//! 回線が切れたときに **ペインもタブもリモートフォルダも失わせず**、回線が戻ったら
//! ユーザーが何もしなくても続きから使えるようにするための判断。**ここは純粋関数だけ**で、
//! 画面の採取・時計・実際の打ち直しは呼び出し側（tako-app）が持つ
//! （[`crate::ssh_progress`] と同じ構え = macOS 上から両言語ぶんを機械検査できる）。
//!
//! # なぜ「一度でも繋がったペイン」だけを対象にするか
//!
//! 到達できないホストで開いたペインを延々と叩き続けると、ユーザーが直しようのない相手へ
//! ネットワーク I/O を撃ち続けることになる。**繋がった実績**があれば「相手は在るのに
//! いま届かない」= 回線側の問題なので、待てば戻る見込みがある。初回の接続失敗は
//! #919 のとおり理由を画面に残して止まる（ペインは残るのでユーザーが打ち直せる）。
//!
//! # 「切れた」と「ユーザーが `exit` した」を混同しない
//!
//! ssh は正常終了でも `Shared connection to <host> closed.` を出すので、**画面の文言だけで
//! 判定してはいけない**（`exit` したペインを勝手に繋ぎ直すと、閉じたい人の邪魔になる）。
//! 見分けの根拠は経路ごとに違う:
//!
//! - `split` / `tab`（#919 のスクリプト経路）= スクリプトが **exit 255 のときだけ**印字する
//!   マーカー行（[`crate::ssh_progress::SCRIPT_FAILURE_MARK`]）
//! - `pane`（既存シェルへ 1 行打つ経路 = #1006）= シェル統合（OSC 133）が返す
//!   **終了コード 255**（[`crate::terminal::CommandState::Failed`]）
//!
//! どちらも「ssh 自身が失敗した」ときにしか立たない（OpenSSH の man: *ssh exits with 255
//! if an error occurred*）。リモートシェルの `exit 1` とも区別できる。

use crate::i18n::Lang;

/// 自動再接続の試行上限。**無限に撃たない**ための頭。
///
/// [`backoff_secs`] と合わせて「切断から約 1 分半で諦める」形にしてある。
/// これより長い断は回線側の復旧作業（VPN の張り直し・移動）を伴うことが多く、
/// そのときは戻ってきたユーザーが自分で打ち直せる状態（ローカルのプロンプト）に
/// なっているほうがよい
pub const MAX_ATTEMPTS: u32 = 6;

/// `attempt` 回目（1 始まり）の再接続を撃つまでに待つ秒数。
///
/// 最初を短くするのは、Wi-Fi の切り替え・スリープ復帰のような**すぐ戻る断**を
/// 待たせないため。後ろを寝かせるのは、戻らない断で撃ち続けないため
pub fn backoff_secs(attempt: u32) -> u64 {
    match attempt {
        0 | 1 => 2,
        2 => 5,
        3 => 10,
        4 => 20,
        _ => 30,
    }
}

/// 切断からここまでの累計待ち秒数（表示と検査用）
pub fn total_backoff_secs(attempts: u32) -> u64 {
    (1..=attempts).map(backoff_secs).sum()
}

/// 次にすること
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// まだ待つ（残り秒数）
    Wait { remaining_secs: u64 },
    /// いま打ち直す（これが `attempt` 回目）
    Retry { attempt: u32 },
    /// 上限に達した。**理由と次の一手を出して静かに止まる**
    GiveUp,
}

/// 直前の試行（まだ 1 度も撃っていなければ 0）から `waited_secs` 秒経ったときに何をするか
pub fn next_step(attempts_done: u32, waited_secs: u64) -> Step {
    if attempts_done >= MAX_ATTEMPTS {
        return Step::GiveUp;
    }
    let attempt = attempts_done + 1;
    let need = backoff_secs(attempt);
    if waited_secs >= need {
        Step::Retry { attempt }
    } else {
        Step::Wait {
            remaining_secs: need - waited_secs,
        }
    }
}

/// 打ち直したあと、結果（繋がった / また失敗した）を待つ上限秒数。
///
/// `ConnectTimeout=10` + `ServerAlive*` ≒ 15 秒なので、失敗なら必ずこの中で決まる。
/// 越えたら「1 回失敗した」と数えて次のバックオフへ進む（永久に結果待ちで固まらない）
pub const ATTEMPT_RESULT_WINDOW_SECS: u64 = 45;

/// 打ち直しの結果待ちが長引きすぎたか
pub fn attempt_timed_out(secs: u64) -> bool {
    secs >= ATTEMPT_RESULT_WINDOW_SECS
}

/// その理由なら**繋ぎ直す意味があるか**。
///
/// 回線側の問題（届かない・落ちた）は待てば戻るので繰り返す価値がある。
/// 一方、鍵・ホスト鍵・設定ファイルの問題は**何度撃っても同じ**で、しかも
/// 相手のログを認証失敗で埋める。ここで切り分けて撃たない
/// （名前解決の失敗は**回線側**に数える: オフラインだと DNS から先に落ちる）
pub fn is_recoverable_reason(reason: Option<&str>) -> bool {
    /// 待っても変わらないもの
    const PERMANENT: &[&str] = &[
        "Permission denied",
        "Too many authentication failures",
        "Host key verification failed",
        "REMOTE HOST IDENTIFICATION HAS CHANGED",
        "Bad configuration option",
        "Bad owner or permissions",
        "no matching host key type",
        "Invalid key length",
    ];
    match reason {
        // 理由が読めないときは繋ぎ直す側に倒す（成立していた接続が切れた形なので）
        None => true,
        Some(r) => !PERMANENT.iter().any(|p| r.contains(p)),
    }
}

/// 自動再接続を仕掛けてよいか。
///
/// `ever_connected` = このペインで一度でも接続が成立したか（モジュール doc 参照）。
///
/// **「画面が動いた」を証拠にしてはいけない**（#1090）: 器（psmux / tmux）や下のシェルも
/// 描くので、[`crate::ssh_progress::ConnectPhase::Opened`] は「沈黙が破れた」までしか
/// 言わない。繋がったことが一度も無いホストへ ssh を打ち直す事故を実機で踏んだ。
/// 呼び出し側は多重化のソケット（接続が張れて初めて作られる）を証拠にする。
/// 多重化を持たないプラットフォームでは証拠が無いので armed にならない
/// （宣言済みの縮退。[`crate::platform::ssh_client::NO_MULTIPLEXING`]）
pub fn should_arm(enabled: bool, ever_connected: bool) -> bool {
    arm_state(enabled, ever_connected).armed()
}

/// 自動再接続の対象か、対象でないなら**なぜか**（#1446）。
///
/// #1040 は `bool` だけを持っていたので、**繋ぎ直さなかった理由が誰にも見えなかった**
/// （画面にも `tako list` にも何も出ず、ペインは黙ってローカルのシェルに残る）。
/// 理由を型で持つと、切断を拾ったのに撃たない場面で**必ず 1 行出せる**ようになり、
/// `ssh_connect.reconnect` としてそのまま CLI / MCP からも読める
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmState {
    /// 対象（切れたら打ち直す）
    Armed,
    /// 対象外: このペインで接続が成立した証拠がまだ無い。
    /// 初回の接続失敗（#919）と、多重化を持たないプラットフォーム
    /// （[`crate::platform::ssh_client::NO_MULTIPLEXING`]）がここへ来る
    NoConnectionEvidence,
    /// 対象外: 自動再接続そのものが切られている（`TAKO_1040_LEGACY`）
    Disabled,
}

impl ArmState {
    /// 対象か（[`should_arm`] の実体）
    pub fn armed(self) -> bool {
        matches!(self, ArmState::Armed)
    }

    /// ワイヤ表記（`tako list` / `read` の `ssh_connect.reconnect.state`）
    pub fn as_str(self) -> &'static str {
        match self {
            ArmState::Armed => "armed",
            ArmState::NoConnectionEvidence => "no_connection_evidence",
            ArmState::Disabled => "disabled",
        }
    }

    /// 対象外の理由（対象なら `None`）。**画面にも応答にも同じ文面を出す**
    pub fn reason(self, lang: Lang) -> Option<&'static str> {
        match (self, lang) {
            (ArmState::Armed, _) => None,
            (ArmState::NoConnectionEvidence, Lang::Ja) => {
                Some("このペインでは接続が成立した記録がありません")
            }
            (ArmState::NoConnectionEvidence, Lang::En) => {
                Some("this pane has no record of a successful connection")
            }
            (ArmState::Disabled, Lang::Ja) => Some("自動再接続が無効です"),
            (ArmState::Disabled, Lang::En) => Some("auto-reconnect is disabled"),
        }
    }
}

/// 対象かどうかと、対象でない理由（[`should_arm`] と同じ材料・1 実装）
pub fn arm_state(enabled: bool, ever_connected: bool) -> ArmState {
    if !enabled {
        // **無効が先**: 切られているなら証拠の有無を論じる意味がない
        ArmState::Disabled
    } else if ever_connected {
        ArmState::Armed
    } else {
        ArmState::NoConnectionEvidence
    }
}

/// 「このペインは SSH だ」をどこから知ったか（#1446）。
///
/// #1446 の根因は**この記憶がプロセスの寿命しか持たなかった**こと。出どころを型で
/// 持つと、`tako list` から「復元で引き継いだのか / 手打ちを引き取ったのか」が読め、
/// 発火しない報告が来たときに**どの入口が抜けたか**を実測せずに切り分けられる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackSource {
    /// `tako open-in remote` / GUI のメニュー・右クリック / MCP（= dispatch 経由）
    Opened,
    /// `layout.json` から復元して引き継いだ（GUI 再起動をまたいだペイン）
    Restored,
    /// #976 の ssh 検知が見つけたので引き取った（ユーザーが手で打ったペイン）
    Detected,
}

impl TrackSource {
    /// ワイヤ表記（`tako list` / `read` の `ssh_connect.reconnect.source`）
    pub fn as_str(self) -> &'static str {
        match self {
            TrackSource::Opened => "opened",
            TrackSource::Restored => "restored",
            TrackSource::Detected => "detected",
        }
    }
}

/// 切断を拾ったのに繋ぎ直さないときに画面へ出す 1 行（#1446）。
///
/// **無言で終わらせない**のが趣旨。#1040 はここで `Failed` にするだけだったので、
/// ユーザーには「ローカルのシェルに戻ります」しか見えず、再接続を待てばいいのか
/// 自分で打つのかが判断できなかった（本 Issue の報告そのもの）
pub fn not_armed_notice(lang: Lang, host: &str, state: ArmState) -> String {
    let why = state.reason(lang).unwrap_or(match lang {
        Lang::Ja => "理由は不明です",
        Lang::En => "reason unknown",
    });
    match lang {
        Lang::Ja => format!(
            "{host} との接続が切れました。自動再接続はしません（{why}）。\
             このペインで ssh {host} を実行すると繋ぎ直せます"
        ),
        Lang::En => format!(
            "Disconnected from {host}. Not reconnecting automatically ({why}). \
             Run ssh {host} in this pane to reconnect"
        ),
    }
}

/// **成立していた接続が壊れたときにしか出ない**行（ssh が印字する英語のまま）。
///
/// `pane` 経路でシェル統合（OSC 133）が届かないペインのための保険。
/// **正常終了（`exit`）で出る `Shared connection to <host> closed.` /
/// `logout` は入れない**（入れると閉じたい人のペインを勝手に繋ぎ直す）
const BROKEN_LINK_PATTERNS: &[&str] = &[
    // 相手が黙った（ServerAlive の見切り）
    "Timeout, server",
    // 送れなくなった
    "client_loop: send disconnect",
    "packet_write_wait",
    "Broken pipe",
    "Connection reset by peer",
    // 相手側から切られた
    "closed by remote host",
];

/// 成立していた接続が壊れたと読める行か
pub fn is_broken_link_line(line: &str) -> bool {
    BROKEN_LINK_PATTERNS.iter().any(|p| line.contains(p))
}

/// 切断の根拠（経路ごとに材料が違う。モジュール doc 参照）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectSignal {
    /// スクリプトのマーカー行が出た（`split` / `tab`）
    ScriptMarker,
    /// シェル統合が ssh の終了コード 255 を返した（`pane`）
    ExitCode,
    /// 接続が壊れたときにしか出ない行が画面に出た。
    /// **シェル統合が届かないペイン向けの保険**（終了コードが読めないときだけ見る）
    BrokenLink,
}

/// 接続が成立したあとのペインで「切れた」と読めるか。
///
/// `new_lines` は**接続成立の時点より後**に出た行だけを渡す。`exit_code` は
/// シェル統合（OSC 133）が返した直近コマンドの終了コード（無ければ `None`）。
///
/// 戻り値の理由は画面から拾った 1 行（読めなければ `None`）で、そのまま
/// `tako list` / `read` の `ssh_connect.reason` に載る
pub fn detect_disconnect(
    new_lines: &[String],
    exit_code: Option<i32>,
) -> Option<(DisconnectSignal, Option<String>)> {
    // ① スクリプト経路: マーカーは exit 255 のときしか出ない。
    //    理由は #919 の文面どおり「その直前の非空行」
    if let Some(idx) = new_lines
        .iter()
        .position(|l| l.contains(crate::ssh_progress::SCRIPT_FAILURE_MARK))
    {
        // #1127: **ssh が出した失敗行を優先して拾う**。素の「直前の非空行」だと、
        // 端末幅で折り返された理由の**尻尾**が理由になる（実機の 44 桁のペインで実測:
        // `testuser@host: Permission denied (publickey).` が 2 行へ割れ、マーカーの
        // 直前の行が `.` の 1 文字だけになった）。理由が読めないと
        // [`is_recoverable_reason`] が「待てば直る」側へ倒れるので、**鍵の失敗でも
        // 打ち直してしまい相手の認証ログを埋める** = この関数が防ぐはずのものが起きる。
        // `ssh_progress::classify` は #1090 で同じ手当てを済ませており、
        // [`crate::ssh_progress::is_ssh_error_line`] はそのために公開されている
        let before = &new_lines[..idx];
        let reason = before
            .iter()
            .map(|l| l.trim())
            .find(|l| crate::ssh_progress::is_ssh_error_line(l))
            .or_else(|| {
                before.iter().rev().map(|l| l.trim()).find(|l| {
                    !l.is_empty() && !l.starts_with(crate::ssh_progress::TAKO_LINE_PREFIX)
                })
            })
            .map(str::to_string);
        return Some((DisconnectSignal::ScriptMarker, reason));
    }
    // ② 既存シェル経路: ssh 自身の失敗だけを見る（#1090 で「255 だけ」から
    //    `is_client_failure` へ。POSIX の `$?` は 0..=255 なので macOS では同値）。
    //    理由は画面の ssh の行から拾う
    if exit_code.is_some_and(crate::remote_fs::is_client_failure) {
        let reason = new_lines
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| crate::ssh_progress::is_ssh_error_line(l))
            .map(str::to_string);
        return Some((DisconnectSignal::ExitCode, reason));
    }
    // ③ 終了コードが読めないペイン（シェル統合が届かない）向けの保険。
    //    **壊れたときにしか出ない行**だけを見るので、`exit` は拾わない
    if exit_code.is_none() {
        if let Some(line) = new_lines
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| is_broken_link_line(l))
        {
            return Some((DisconnectSignal::BrokenLink, Some(line.to_string())));
        }
    }
    None
}

/// 繰り返しても同じ理由（鍵・ホスト鍵・設定）で繋ぎ直さないときの 1 行（#1446）。
///
/// [`is_recoverable_reason`] が false を返す場面。理由そのものは ssh が画面へ
/// 生で出しているので、ここが言うのは**繰り返さないという判断**のほう
pub fn permanent_notice(lang: Lang, host: &str, reason: Option<&str>) -> String {
    let detail = reason.unwrap_or(match lang {
        Lang::Ja => "理由は上の行です",
        Lang::En => "the reason is printed above",
    });
    match lang {
        Lang::Ja => format!(
            "{host} との接続が切れました。何度試しても同じ理由なので自動再接続はしません（{detail}）"
        ),
        Lang::En => format!(
            "Disconnected from {host}. Not retrying: the same error would repeat ({detail})"
        ),
    }
}

/// 再接続を始めるときにペインへ出す一言（`tako list` の `reason` ではなくチップの文言）
pub fn reconnecting_label(lang: Lang, host: &str, attempt: u32) -> String {
    match lang {
        Lang::Ja => format!("{host} へ再接続しています…（{attempt}/{MAX_ATTEMPTS}）"),
        Lang::En => format!("Reconnecting to {host}… ({attempt}/{MAX_ATTEMPTS})"),
    }
}

/// 待機中の一言（秒が動くので「生きている」ことが伝わる）
pub fn waiting_label(lang: Lang, host: &str, attempt: u32, remaining_secs: u64) -> String {
    match lang {
        Lang::Ja => {
            format!("{host} へ再接続します（{attempt}/{MAX_ATTEMPTS}・あと {remaining_secs}s）")
        }
        Lang::En => {
            format!("Reconnecting to {host} in {remaining_secs}s ({attempt}/{MAX_ATTEMPTS})")
        }
    }
}

/// 諦めたときの「理由 + 次の一手」。**静かに止まる**ための文面なので、
/// 何が起きたかと、ユーザーが次に何をすればいいかを 1 行で両方言う
pub fn gave_up_label(lang: Lang, host: &str) -> String {
    match lang {
        Lang::Ja => format!(
            "{host} へ再接続できませんでした（{MAX_ATTEMPTS} 回）。\
             回線が戻ったらこのペインで ssh {host} を実行してください"
        ),
        Lang::En => format!(
            "Could not reconnect to {host} after {MAX_ATTEMPTS} tries. \
             Run ssh {host} in this pane once the network is back"
        ),
    }
}

/// ユーザーが自分で打ち始めたので降りる、と伝える一言
pub fn cancelled_label(lang: Lang, host: &str) -> String {
    match lang {
        Lang::Ja => format!("{host} への自動再接続を中止しました（入力を検知）"),
        Lang::En => format!("Stopped auto-reconnecting to {host} (you started typing)"),
    }
}

// --- リモートフォルダ / 保留中の書き戻しの自動復帰（#1040 要件 3）-----------------

/// ツリーのリモートフォルダを繋ぎ直しに行く間隔（秒）。
///
/// ペインと違って**打ち直す先が無い**ので、こちらは tako が自分で接続を試す
/// = ネットワーク I/O が出る。切断を観測している間だけ・この間隔で・
/// [`FOLDER_MAX_PROBES`] 回までに抑える
pub fn folder_probe_secs(probes_done: u32) -> u64 {
    match probes_done {
        0 => 5,
        1 => 10,
        2 => 20,
        3 => 40,
        4 => 60,
        _ => 120,
    }
}

/// リモートフォルダの自動復帰を諦めるまでの試行回数。
/// 諦めたあとは行の「切断」バッジと右クリックの「再読み込み」が残る（#976 の従来動作）
pub const FOLDER_MAX_PROBES: u32 = 8;

/// フォルダの自動復帰を試すか（`probes_done` 回済み・前回から `waited_secs` 秒）
pub fn folder_should_probe(probes_done: u32, waited_secs: u64) -> bool {
    probes_done < FOLDER_MAX_PROBES && waited_secs >= folder_probe_secs(probes_done)
}

/// フォルダが自動で戻ったときの通知
pub fn folder_restored(lang: Lang, host: &str) -> String {
    match lang {
        Lang::Ja => format!("{host} へ再接続しました（フォルダを読み直しました）"),
        Lang::En => format!("Reconnected to {host} (folders reloaded)"),
    }
}

/// 保留中の書き戻しを復帰時に押し出せたときの通知（#966 の pending）
pub fn pending_pushed(lang: Lang, host: &str, count: usize) -> String {
    match lang {
        Lang::Ja => format!("{host} へ保留していた保存 {count} 件を送りました"),
        Lang::En => format!("Pushed {count} pending save(s) to {host}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 最初の再接続は待たせない() {
        // Wi-Fi 切替やスリープ復帰は数秒で戻る。そこを長く待たせない
        assert_eq!(backoff_secs(1), 2);
        assert!(backoff_secs(1) < backoff_secs(MAX_ATTEMPTS));
    }

    #[test]
    fn バックオフは単調非減少で頭打ちになる() {
        let mut prev = 0;
        for a in 1..=(MAX_ATTEMPTS + 3) {
            let v = backoff_secs(a);
            assert!(v >= prev, "attempt={a} で減っている");
            assert!(v <= 30, "attempt={a} が長すぎる");
            prev = v;
        }
    }

    #[test]
    fn 待ち時間が足りなければ待つ() {
        assert_eq!(next_step(0, 0), Step::Wait { remaining_secs: 2 });
        assert_eq!(next_step(0, 1), Step::Wait { remaining_secs: 1 });
        assert_eq!(next_step(0, 2), Step::Retry { attempt: 1 });
        assert_eq!(next_step(1, 5), Step::Retry { attempt: 2 });
    }

    #[test]
    fn 上限に達したら諦める() {
        assert_eq!(next_step(MAX_ATTEMPTS, 999), Step::GiveUp);
        assert_eq!(
            next_step(MAX_ATTEMPTS - 1, 999),
            Step::Retry {
                attempt: MAX_ATTEMPTS
            }
        );
    }

    #[test]
    fn 全部撃っても二分以内に諦める() {
        // 「戻らない断」で延々と待たせない（諦めたらユーザーが自分で打てる）
        assert!(
            total_backoff_secs(MAX_ATTEMPTS) <= 120,
            "累計 {}s は長すぎる",
            total_backoff_secs(MAX_ATTEMPTS)
        );
    }

    /// #1127: **折り返しで割れた理由**でも認証系だと分かる。
    ///
    /// 実機（Windows・44 桁のペイン）の画面をそのまま入れる。`.` の 1 文字だけの
    /// 行がマーカーの直前に来るので、素の「直前の非空行」だと理由が `.` になり
    /// 「待てば直る」へ倒れて**鍵の失敗を打ち直してしまう**
    #[test]
    fn 折り返しで割れた理由でも認証系だと分かる() {
        let screen: Vec<String> = [
            "TAKO_1040_BACK",
            "PS C:\\Users\\winuser> echo 'testuser@host: Perm",
            "ission denied (publickey).' 'tako: selftest-",
            "nonexistent-1040 への接続に失敗しました（ssh",
            " exit ）。理由は上の行です'",
            "testuser@host: Permission denied (publickey)",
            ".",
            "tako: selftest-nonexistent-1040 への接続に失",
            "敗しました（ssh exit ）。理由は上の行です",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (signal, reason) = detect_disconnect(&screen, None).expect("切断を検知する");
        assert_eq!(signal, DisconnectSignal::ScriptMarker);
        assert_eq!(
            reason.as_deref(),
            Some("testuser@host: Permission denied (publickey)"),
            "折り返しの尻尾（`.`）を理由にしている"
        );
        assert!(
            !is_recoverable_reason(reason.as_deref()),
            "認証系なのに打ち直す側へ倒れている"
        );
    }

    /// 折り返していない従来の形（= #919 の契約）は 1 文字も変わらない
    #[test]
    fn 折り返していない画面では直前の行がそのまま理由になる() {
        let screen: Vec<String> = [
            "ssh: connect to host h port 22: Connection refused",
            "tako: h への接続に失敗しました（ssh exit 255）。理由は上の行です",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (_, reason) = detect_disconnect(&screen, None).expect("切断を検知する");
        assert_eq!(
            reason.as_deref(),
            Some("ssh: connect to host h port 22: Connection refused")
        );
        assert!(is_recoverable_reason(reason.as_deref()), "回線側は繰り返す");
    }

    /// ssh の目印がどこにも無ければ従来どおり「直前の非空行」へ落ちる
    #[test]
    fn 目印が無ければ直前の非空行へ落ちる() {
        let screen: Vec<String> = [
            "something unusual happened",
            "",
            "tako: h への接続に失敗しました（ssh exit 255）。理由は上の行です",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (_, reason) = detect_disconnect(&screen, None).expect("切断を検知する");
        assert_eq!(reason.as_deref(), Some("something unusual happened"));
    }

    #[test]
    fn 認証やホスト鍵の失敗は繰り返さない() {
        // 何度撃っても同じで、相手のログを認証失敗で埋めるだけ
        for r in [
            "testuser@win: Permission denied (publickey).",
            "Host key verification failed.",
            "@@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@@",
            "Bad owner or permissions on /Users/testuser/.ssh/config",
        ] {
            assert!(!is_recoverable_reason(Some(r)), "繰り返してしまう: {r}");
        }
    }

    #[test]
    fn 回線側の失敗は繰り返す価値がある() {
        for r in [
            "ssh: connect to host win port 22: Operation timed out",
            "Timeout, server win not responding.",
            "client_loop: send disconnect: Broken pipe",
            // オフラインだと DNS から先に落ちる = 回線側
            "ssh: Could not resolve hostname win: nodename nor servname provided",
            "Shared connection to win closed.",
        ] {
            assert!(is_recoverable_reason(Some(r)), "諦めてしまう: {r}");
        }
        // 理由が読めないときは繋ぎ直す側へ倒す
        assert!(is_recoverable_reason(None));
    }

    #[test]
    fn 結果待ちは有限で打ち切る() {
        assert!(!attempt_timed_out(ATTEMPT_RESULT_WINDOW_SECS - 1));
        assert!(attempt_timed_out(ATTEMPT_RESULT_WINDOW_SECS));
        // ssh 自身の見切り（ConnectTimeout + ServerAlive）より必ず長い
        assert!(
            ATTEMPT_RESULT_WINDOW_SECS
                > (crate::remote_fs::CONNECT_TIMEOUT_SECS
                    + crate::remote_fs::SERVER_ALIVE_INTERVAL_SECS
                        * crate::remote_fs::SERVER_ALIVE_COUNT_MAX) as u64
        );
    }

    #[test]
    fn 一度も繋がっていないペインは対象にしない() {
        assert!(!should_arm(true, false));
        assert!(should_arm(true, true));
        assert!(!should_arm(false, true));
    }

    #[test]
    fn スクリプトのマーカーで切断と読める() {
        let l = lines(&[
            "[user@remote ~]$ ",
            "Shared connection to 127.0.0.1 closed.",
            "tako: win への接続に失敗しました（ssh exit 255）。理由は上の行です",
        ]);
        let (sig, reason) = detect_disconnect(&l, None).expect("切断として読めていない");
        assert_eq!(sig, DisconnectSignal::ScriptMarker);
        assert_eq!(reason.unwrap(), "Shared connection to 127.0.0.1 closed.");
    }

    #[test]
    fn ユーザーがexitしただけなら切断にしない() {
        // ssh は正常終了でも同じ行を出す。**文言だけで判定すると誤爆する**
        let l = lines(&[
            "[user@remote ~]$ exit",
            "logout",
            "Shared connection to win closed.",
        ]);
        assert_eq!(detect_disconnect(&l, None), None);
        assert_eq!(detect_disconnect(&l, Some(0)), None);
    }

    #[test]
    fn 既存シェル経路は終了コード255で切断と読める() {
        let l = lines(&[
            "user@mac ~ % ssh win",
            "client_loop: send disconnect: Broken pipe",
            "user@mac ~ %",
        ]);
        let (sig, reason) = detect_disconnect(&l, Some(255)).expect("切断として読めていない");
        assert_eq!(sig, DisconnectSignal::ExitCode);
        assert!(
            reason.is_none()
                || reason.as_deref() == Some("client_loop: send disconnect: Broken pipe")
        );
    }

    #[test]
    fn シェル統合が無いペインは壊れた行で切断と読める() {
        // OSC 133 が届かないペイン（`state: unknown` / `exit_code: null`）。
        // 実測でこの形に当たった（検証用シェルが zsh として認識されない構成）
        for raw in [
            "Timeout, server testremote not responding.",
            "client_loop: send disconnect: Broken pipe",
            "Connection to win closed by remote host.",
        ] {
            let l = lines(&["[user@remote ~]$ ", raw, "user@mac ~ %"]);
            let (sig, reason) = detect_disconnect(&l, None).expect("切断として読めていない");
            assert_eq!(sig, DisconnectSignal::BrokenLink);
            assert_eq!(reason.unwrap(), raw);
        }
    }

    #[test]
    fn 保険の行判定はexitの正常終了を拾わない() {
        // `exit` で出る行を混ぜても切断にしない（閉じたい人の邪魔をしない）
        let l = lines(&[
            "[user@remote ~]$ exit",
            "logout",
            "Shared connection to win closed.",
        ]);
        assert_eq!(detect_disconnect(&l, None), None);
        for line in ["logout", "Shared connection to win closed."] {
            assert!(!is_broken_link_line(line), "拾ってしまう: {line}");
        }
    }

    #[test]
    fn 終了コードが読めるペインでは保険を使わない() {
        // 統合があるなら**終了コードが正**（別のホストへの ssh が失敗した行などを拾わない）
        let l = lines(&["client_loop: send disconnect: Broken pipe"]);
        assert_eq!(detect_disconnect(&l, Some(0)), None);
        assert_eq!(detect_disconnect(&l, Some(1)), None);
    }

    #[test]
    fn リモートのexit1を接続の失敗と読まない() {
        // ssh 自身の失敗だけが 255（OpenSSH の man）
        let l = lines(&["user@mac ~ % ssh win false", "user@mac ~ %"]);
        assert_eq!(detect_disconnect(&l, Some(1)), None);
    }

    #[test]
    fn 理由が読めなくても切断としては読める() {
        let l = lines(&["tako: win への接続に失敗しました（ssh exit 255）。理由は上の行です"]);
        let (sig, reason) = detect_disconnect(&l, None).unwrap();
        assert_eq!(sig, DisconnectSignal::ScriptMarker);
        assert_eq!(reason, None);
    }

    #[test]
    fn 文面は日英とも空でなくホスト名を含む() {
        for lang in [Lang::Ja, Lang::En] {
            for s in [
                reconnecting_label(lang, "win", 1),
                waiting_label(lang, "win", 2, 5),
                gave_up_label(lang, "win"),
                cancelled_label(lang, "win"),
                folder_restored(lang, "win"),
                pending_pushed(lang, "win", 3),
            ] {
                assert!(!s.trim().is_empty());
                assert!(s.contains("win"), "ホスト名が無い: {s}");
            }
        }
    }

    #[test]
    fn 諦めの文面は次の一手を必ず持つ() {
        // 「静かに止まる」= 何もしないではなく「次に何をすればいいか」を残す（#919 の原則）
        for lang in [Lang::Ja, Lang::En] {
            let s = gave_up_label(lang, "win");
            assert!(s.contains("ssh win"), "次の一手が無い: {s}");
        }
    }

    #[test]
    fn フォルダの再接続も間隔が伸びて頭打ちになる() {
        let mut prev = 0;
        for p in 0..(FOLDER_MAX_PROBES + 2) {
            let v = folder_probe_secs(p);
            assert!(v >= prev);
            assert!(v <= 120);
            prev = v;
        }
        assert!(folder_should_probe(0, 5));
        assert!(!folder_should_probe(0, 4));
        assert!(
            !folder_should_probe(FOLDER_MAX_PROBES, 9999),
            "上限を越えて試している"
        );
    }

    // --- #1446: 対象かどうかと、対象でない理由 ---------------------------------

    #[test]
    fn 対象かの答えはshould_armとarm_stateで食い違わない() {
        // `should_arm` は `arm_state` の薄い包み（1 実装）。真理値表の全点で一致させる
        for enabled in [true, false] {
            for ever in [true, false] {
                assert_eq!(
                    should_arm(enabled, ever),
                    arm_state(enabled, ever).armed(),
                    "enabled={enabled} ever_connected={ever} で食い違った"
                );
            }
        }
        assert_eq!(arm_state(true, true), ArmState::Armed);
        assert_eq!(arm_state(true, false), ArmState::NoConnectionEvidence);
        // **無効が先**: 切られているなら証拠の有無を論じない（画面の文面もそうなる）
        assert_eq!(arm_state(false, true), ArmState::Disabled);
        assert_eq!(arm_state(false, false), ArmState::Disabled);
    }

    #[test]
    fn 対象でない理由は必ず言葉になる() {
        // #1446 の本体: 「繋ぎ直さない」を**無言にしない**ので、
        // 対象外の状態はどちらの言語でも必ず理由を持つ
        for state in [ArmState::NoConnectionEvidence, ArmState::Disabled] {
            for lang in [Lang::Ja, Lang::En] {
                let why = state.reason(lang);
                assert!(why.is_some(), "{state:?} / {lang:?} の理由が無い");
                assert!(!why.unwrap().trim().is_empty());
            }
            assert_ne!(state.as_str(), "armed");
        }
        // 対象なら理由は無い（「繋ぎ直します」に理由は要らない）
        assert!(ArmState::Armed.reason(Lang::Ja).is_none());
        assert!(ArmState::Armed.reason(Lang::En).is_none());
    }

    #[test]
    fn 状態と出どころのワイヤ表記は重複しない() {
        let arms = [
            ArmState::Armed,
            ArmState::NoConnectionEvidence,
            ArmState::Disabled,
        ];
        let mut wire: Vec<&str> = arms.iter().map(|a| a.as_str()).collect();
        wire.sort_unstable();
        wire.dedup();
        assert_eq!(
            wire.len(),
            arms.len(),
            "ArmState のワイヤ表記が衝突している"
        );

        let sources = [
            TrackSource::Opened,
            TrackSource::Restored,
            TrackSource::Detected,
        ];
        let mut wire: Vec<&str> = sources.iter().map(|s| s.as_str()).collect();
        wire.sort_unstable();
        wire.dedup();
        assert_eq!(
            wire.len(),
            sources.len(),
            "TrackSource のワイヤ表記が衝突している"
        );
    }

    #[test]
    fn 繋ぎ直さないときの案内は次の一手を必ず含む() {
        // 画面に出るのはこの 1 行だけなので、**何が起きたか**と
        // **次に何をすればいいか**の両方が入っていること（#1446 / #919 と同じ物差し）
        for lang in [Lang::Ja, Lang::En] {
            for state in [ArmState::NoConnectionEvidence, ArmState::Disabled] {
                let text = not_armed_notice(lang, "example-host", state);
                assert!(text.contains("example-host"), "相手の名前が無い: {text}");
                assert!(
                    text.contains("ssh example-host"),
                    "次の一手（打ち直す 1 行）が無い: {text}"
                );
                assert!(
                    text.contains(state.reason(lang).expect("対象外には理由がある")),
                    "理由が入っていない: {text}"
                );
            }
            // 待っても変わらない理由のほうは、**繰り返さない判断**を言う
            let text = permanent_notice(lang, "example-host", Some("Permission denied"));
            assert!(text.contains("example-host"));
            assert!(
                text.contains("Permission denied"),
                "拾った理由が消えた: {text}"
            );
            // 理由が読めなくても無言にはしない
            let blind = permanent_notice(lang, "example-host", None);
            assert!(!blind.trim().is_empty());
            assert!(blind.contains("example-host"));
        }
    }

    #[test]
    fn 繋ぎ直す価値のある理由と無い理由を取り違えない() {
        // #1446 で通知の文面を 2 種類に分けた根拠。ここが入れ替わると
        // 鍵の失敗に「回線が戻ったら」と案内してしまう
        assert!(!is_recoverable_reason(Some(
            "testuser@example-host: Permission denied (publickey)."
        )));
        assert!(is_recoverable_reason(Some(
            "client_loop: send disconnect: Broken pipe"
        )));
        assert!(is_recoverable_reason(None));
    }
}

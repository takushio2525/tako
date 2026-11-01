//! remote_starting — daemon を起こしている途中を status へ知らせる（#2608）
//!
//! 起動の 1 実装 `remote::spawn_daemon`（`tako remote start` / 起動ボタン / MCP / GUI 起動時の
//! 自動復帰 #1485）は、旧版の停止（#2518）→ 新版の起動 → 新版が待ち受けを開いて port ファイルを
//! 書く → tailscale の serve を張る → PID ファイルを書く → 起動情報を返す、の順に進む。
//! 待ち受けを開いてから PID ファイルを書くまでは tailscale の応答次第で数秒〜（#2466 の実測は 8 秒）。
//!
//! その間に別のプロセスが `tako remote status` を読むと「PID ファイルが無いのに記録された待ち受けが
//! 応答している」= #2455 の「PID ファイルだけが消えた daemon」と同じ形に見え、`tako remote stop` で
//! 止めてと案内していた（#2608。11/01 の本番で install 後の GUI の再起動直後に観察）。案内どおり
//! 止めると、立ち上がったばかりの新版を止める。status は**起こしている途中を知る手段を持っていなかった**
//! （`spawn_daemon` は途中の印を残さず、`last_autostart` は終わった後にしか書かれない）。
//!
//! ここが持つのは 3 つ:
//!
//! 1. **印の形**（[`StartingMarker`]。`<state_dir>/tako-remote.starting`）。書くのは `spawn_daemon`
//!    だけで、抜けるときに消える（`remote::StartingGuard` の Drop = Err の早期 return も panic の巻き戻しも）
//! 2. **印がまだ効いているかの判定**（[`is_active`]。純関数）。消し損ねても「起動中」のまま残らない:
//!    書いたプロセスが死んでいる・印より後に起動した別のプロセスが pid を再利用している・
//!    [`TTL_SECS`] を過ぎた印は効かない（status は #2455 の `pidless_listener` と stop の案内へ戻る）
//! 3. **status へ載せる形**（[`status_field`] / [`detail`]）。読む側（CLI・MCP）は再計算しない
//!
//! 印が効いている間に**明示の start / stop が来たら断る**（#2613。断りの文は
//! `RemoteRefusal::DaemonStarting` / `DaemonStartingStop`、誰が何をしている途中かの 1 文は [`doing_in`]）。
//! 区間内の start は、入れ替えの途中の旧版を自分も「入れ替える相手」と読んで止めに行き、子を起こして
//! daemon を 2 本にする（PID ファイルに載らない孤児が残る）か、印を上書きして抜けるときに消す
//! （status が stop の案内へ戻る）。区間内の stop は、止めても進行中の起動が新版を起こし直すか、
//! PID ファイルも管理トークンも無いので止められない（どちらも段 1 で実測）。
//!
//! 短命な稼働状態なので #916 の移行台帳には載せない（`LastAutostart` / `ServeHealth` と同じ。
//! 次の起動で必ず書き直され、読めなければ「印が無い」として扱えばよいだけ）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tako_core::i18n::Lang;

/// A/B: #2608 前へ戻す（status が印を見ない = 起こしている途中も `pidless_listener` と stop の案内）
pub const LEGACY_ENV: &str = "TAKO_2608_LEGACY";

/// A/B: #2613 前へ戻す（印が効いていても明示の start / stop が断らない = 2 本目の daemon を起こしうる）。
/// status の A/B（[`LEGACY_ENV`]）とは分ける（片方だけを戻して比べられるように）
pub const START_LEGACY_ENV: &str = "TAKO_2613_LEGACY";

/// 印が効く上限（秒）。`spawn_daemon` が抜けるまでの最長より長くとる:
/// 旧版の停止（管理 API 3 秒 + 終了待ち 5 秒 + 撃って 5 秒）+ 起動情報の待ち 30 秒 + 子の終了待ち 2 秒
/// ≒ 45 秒。書いたプロセスが固まったまま生きている回だけがこの上限で落ちる
pub const TTL_SECS: u64 = 90;

/// 起こしている途中（入れ替えではない）
pub const STATE_STARTING: &str = "starting";
/// 旧版を止めて新版を起こしている途中（#2518 の入れ替え）
pub const STATE_REPLACING: &str = "replacing";

/// #2608 前の status へ戻すか
pub fn legacy_mode() -> bool {
    std::env::var(LEGACY_ENV).is_ok_and(|v| v == "1" || v == "true" || v == "on")
}

/// #2613 前の明示の start / stop へ戻すか
pub fn start_legacy_mode() -> bool {
    std::env::var(START_LEGACY_ENV).is_ok_and(|v| v == "1" || v == "true" || v == "on")
}

/// 「daemon を起こしている途中」の印（`<state_dir>/tako-remote.starting`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartingMarker {
    /// 書いた時刻（unix epoch 秒）
    pub at: u64,
    /// 書いたプロセス（= `spawn_daemon` を呼んでいる GUI / CLI）
    pub pid: u32,
    /// 頼んだ経路（`remote::RequestOrigin::as_str`。名乗らなければ `unspecified`。#2533）
    #[serde(default)]
    pub via: String,
    /// 入れ替えなら止める旧版の daemon の pid（#2518）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_pid: Option<u32>,
}

impl StartingMarker {
    /// [`STATE_REPLACING`] / [`STATE_STARTING`]
    pub fn state(&self) -> &'static str {
        if self.previous_pid.is_some() {
            STATE_REPLACING
        } else {
            STATE_STARTING
        }
    }
}

/// 印がまだ効いているか（純関数。材料の I/O は呼ぶ側 = `remote::start_in_progress`）。
///
/// - `writer_alive`: 書いたプロセス（`marker.pid`）が生きているか
/// - `writer_started`: そのプロセスの起動時刻（unix 秒。取れない OS は `None`）。
///   印より後に起動していれば pid を再利用した別のプロセス（判定は #2466 の片側判定と同じ 1 実装）
///
/// 時計が戻った（印の時刻が今より先）ときも、先へずれた分が [`TTL_SECS`] を超えれば効かせない
/// （壊れた時刻の印がいつまでも効き続けない）
pub fn is_active(
    marker: &StartingMarker,
    now: u64,
    writer_alive: bool,
    writer_started: Option<u64>,
) -> bool {
    if !writer_alive {
        return false;
    }
    if let Some(started) = writer_started {
        if !tako_core::platform::procinfo::started_before_record(started, marker.at) {
            return false;
        }
    }
    now.saturating_sub(marker.at) <= TTL_SECS && marker.at.saturating_sub(now) <= TTL_SECS
}

/// 頼んだ経路を人が読む名前へ（既知の値だけ。#2533 の経路の語彙）。
/// (経路, 言語) の網羅 match で、英語は文の途中に置く形（小文字で始める）
fn via_label_in(via: &str, lang: Lang) -> &'static str {
    match (via, lang) {
        ("autostart", Lang::Ja) => "GUI 起動時の自動復帰",
        ("autostart", Lang::En) => "the GUI's automatic restore at launch",
        ("cli", _) => "`tako remote start`",
        ("dispatch", Lang::Ja) => "MCP / GUI からの起動の要求",
        ("dispatch", Lang::En) => "a start request from MCP / the GUI",
        ("panel", Lang::Ja) => "リモートカードの起動ボタン",
        ("panel", Lang::En) => "the start button of the remote card",
        (_, Lang::Ja) => "起動の要求",
        (_, Lang::En) => "a start request",
    }
}

/// 誰が何をしている途中かの 1 文（句点なし。status の [`detail`] と #2613 の断りが共有する）。
/// 日本語は #2608 の `detail` の前半とバイト一致
pub fn doing_in(marker: &StartingMarker, now: u64, lang: Lang) -> String {
    let elapsed = now.saturating_sub(marker.at);
    let who = via_label_in(&marker.via, lang);
    match (marker.previous_pid, lang) {
        (Some(pid), Lang::Ja) => format!(
            "{who}がリモート daemon を新版へ入れ替えている途中です（旧版の PID {pid}・開始から {elapsed} 秒）"
        ),
        (Some(pid), Lang::En) => format!(
            "The remote daemon is being replaced with the new version by {who} \
             (old PID {pid}, started {elapsed} s ago)"
        ),
        (None, Lang::Ja) => {
            format!("{who}がリモート daemon を起動している途中です（開始から {elapsed} 秒）")
        }
        (None, Lang::En) => {
            format!("The remote daemon is being started by {who} (started {elapsed} s ago)")
        }
    }
}

/// 人が読む 1 行（`warnings` と CLI の stderr に出す）。**stop を案内しない**のが要点
pub fn detail(marker: &StartingMarker, now: u64) -> String {
    let doing = doing_in(marker, now, Lang::Ja);
    format!(
        "{doing}。新しい daemon は待ち受けを開いてから PID ファイルを書くまで数十秒かかることがあり、\
         その間は `running: false` に見えます。止めたり起動し直したりせず、しばらく待ってから \
         `tako remote status` を読み直してください"
    )
}

/// `tako remote status` / MCP `tako_remote_status` の `starting`（純関数）。
/// `listener` は記録された待ち受けが既に応答していればその到達先（新しい daemon のもの）
pub fn status_field(marker: &StartingMarker, now: u64, listener: Option<&str>) -> Value {
    let mut out = json!({
        "state": marker.state(),
        "since": marker.at,
        "elapsed_secs": now.saturating_sub(marker.at),
        "via": marker.via,
        "detail": detail(marker, now),
    });
    if let Some(pid) = marker.previous_pid {
        out["previous_pid"] = json!(pid);
    }
    if let Some(l) = listener {
        out["listener"] = json!(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(at: u64, previous_pid: Option<u32>) -> StartingMarker {
        StartingMarker {
            at,
            pid: 4242,
            via: "autostart".to_string(),
            previous_pid,
        }
    }

    #[test]
    fn 書き手が生きていて期限内なら効く() {
        let m = marker(1000, Some(7));
        assert!(is_active(&m, 1000, true, Some(990)));
        assert!(is_active(&m, 1000 + TTL_SECS, true, None));
    }

    #[test]
    fn 書き手が死んでいれば効かない() {
        assert!(!is_active(&marker(1000, None), 1001, false, None));
    }

    #[test]
    fn 期限を過ぎた印は効かない() {
        assert!(!is_active(&marker(1000, None), 1001 + TTL_SECS, true, None));
    }

    #[test]
    fn 印より後に起動したプロセスはpidの再利用で書き手ではない() {
        let slack = tako_core::platform::procinfo::START_TIME_SLACK_SECS;
        let m = marker(1000, None);
        assert!(!is_active(&m, 1030, true, Some(1001 + slack)));
        assert!(is_active(&m, 1030, true, Some(1000 + slack)));
    }

    #[test]
    fn 先の時刻へ大きくずれた印は効かない() {
        let m = marker(5000, None);
        assert!(is_active(&m, 5000 - TTL_SECS, true, None));
        assert!(!is_active(&m, 4999 - TTL_SECS, true, None));
    }

    #[test]
    fn 入れ替えと起動を見分けて載せる() {
        let replacing = status_field(&marker(1000, Some(77)), 1007, Some("127.0.0.1:1"));
        assert_eq!(replacing["state"], STATE_REPLACING);
        assert_eq!(replacing["previous_pid"], 77);
        assert_eq!(replacing["elapsed_secs"], 7);
        assert_eq!(replacing["via"], "autostart");
        assert_eq!(replacing["listener"], "127.0.0.1:1");
        let starting = status_field(&marker(1000, None), 1000, None);
        assert_eq!(starting["state"], STATE_STARTING);
        assert!(starting.get("previous_pid").is_none());
        assert!(starting.get("listener").is_none());
    }

    #[test]
    fn 案内は止めさせない() {
        for m in [marker(1000, Some(77)), marker(1000, None)] {
            let line = detail(&m, 1003);
            assert!(!line.contains("tako remote stop"), "{line}");
            assert!(!line.contains("tako remote start"), "{line}");
            assert!(line.contains("tako remote status"), "{line}");
            assert!(line.contains("GUI 起動時の自動復帰"), "{line}");
        }
    }

    #[test]
    fn 経路の名前は既知の値だけ() {
        for (via, want) in [
            ("autostart", "GUI 起動時の自動復帰"),
            ("cli", "`tako remote start`"),
            ("dispatch", "MCP / GUI からの起動の要求"),
            ("panel", "リモートカードの起動ボタン"),
            ("unspecified", "起動の要求"),
            ("<script>", "起動の要求"),
        ] {
            assert_eq!(via_label_in(via, Lang::Ja), want);
        }
        for (via, want) in [
            ("autostart", "the GUI's automatic restore at launch"),
            ("cli", "`tako remote start`"),
            ("dispatch", "a start request from MCP / the GUI"),
            ("panel", "the start button of the remote card"),
            ("<script>", "a start request"),
        ] {
            assert_eq!(via_label_in(via, Lang::En), want);
        }
    }

    #[test]
    fn 途中の1文は日英を持ち英語に日本語が混ざらない() {
        for m in [marker(1000, Some(77)), marker(1000, None)] {
            let ja = doing_in(&m, 1004, Lang::Ja);
            let en = doing_in(&m, 1004, Lang::En);
            assert!(detail(&m, 1004).starts_with(&format!("{ja}。")), "{ja}");
            assert!(en.is_ascii(), "{en}");
            assert!(en.contains("4 s ago"), "{en}");
            assert!(en.contains("the GUI's automatic restore at launch"), "{en}");
        }
        assert!(doing_in(&marker(1000, Some(77)), 1000, Lang::En).contains("old PID 77"));
        assert!(doing_in(&marker(1000, None), 1000, Lang::En).contains("being started"));
    }

    #[test]
    fn 印はjsonを往復し最小の形からも読める() {
        let m = marker(1000, Some(3));
        let back: StartingMarker =
            serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        let min: StartingMarker = serde_json::from_str(r#"{"at":1,"pid":2}"#).unwrap();
        assert_eq!(min.via, "");
        assert_eq!(min.previous_pid, None);
        assert_eq!(min.state(), STATE_STARTING);
    }
}

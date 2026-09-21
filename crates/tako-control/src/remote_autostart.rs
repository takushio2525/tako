//! remote_autostart — 「起動していた」を覚えて GUI 起動時に立て直す（#1485）
//!
//! remote daemon は GUI とは別プロセス（`tako remote serve`）なので、Mac を再起動
//! すると消える。タブ・ペイン・プレビュー・SSH の追跡（#1446）は persist が戻すのに、
//! remote だけは**誰も立て直さない**まま `running: false` で止まっていた。スマホからは
//! 「PWA が繋がらない」としか見えず、PC 側にも何も出ない。
//!
//! ここが引き受けるのは 3 つ:
//!
//! 1. **意図の永続** — `tako remote start` が成功したら「起動していた」を残し、
//!    `tako remote stop` で消す（**ユーザーの明示操作だけが書く**）
//! 2. **判断** — GUI 起動時に立て直すか、立て直さないならなぜかを 1 つの純粋関数
//!    ([`autostart_decision`]) が返す。読む側（CLI・チップ・persist.log）は
//!    **再計算せず状態を読むだけ**（#372 / #1473 と同じ理屈）
//! 3. **記録** — 立て直した / 立て直せなかった（理由）を残し、`tako remote status` /
//!    MCP `tako_remote_status` から読めるようにする（#1399 / #1446 ④ の「無言にしない」）
//!
//! 実際に daemon を起こすのは GUI（`tako-app`）で、`spawn_daemon` を background で
//! 呼ぶ。ここは**判断と記録の 1 実装**だけを持つので、実 tailscale なしで
//! 「desired あり → 起こす / stop 済み → 何もしない / 失敗 → 理由を残して再試行」の
//! 全経路を機械検証できる。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A/B: #1485 前へ戻す（「起動していた」を覚えず、GUI 起動時に何もしない）。
/// 立てると再起動後に `running: false` のままになる = Issue の症状そのもの
pub const LEGACY_ENV: &str = "TAKO_1485_LEGACY";
/// 検証用: 各試行の待ちを固定の秒数へ上書きする（下限 1 秒）。
/// tailscaled を落として上げ直す実測を現実的な時間で回すため
pub const BACKOFF_SECS_ENV: &str = "TAKO_1485_BACKOFF_SECS";
/// 検証用: 試行回数の上書き（1..=20 へ丸める）
pub const ATTEMPTS_ENV: &str = "TAKO_1485_ATTEMPTS";

/// 試行ごとの待ち（秒）。**1 回目の前にも待つ**。
///
/// Mac の再起動直後は tailscaled（standalone / GUI 版のどちらも）が上がりきって
/// おらず、1 発目は高確率で「Tailscale が見つからない」で落ちる。最後の試行まで
/// 合計 77 秒あるので、ログイン直後の立ち上がりを跨げる。
/// 表を伸ばすより**回数で止める**のは、諦めた理由を表に出したいため（#1049 の
/// re-assert 上限と同じ考え方）
pub const BACKOFF_SECS: &[u64] = &[2, 5, 10, 20, 40];

/// 試行回数の既定（= [`BACKOFF_SECS`] の長さ）
pub const ATTEMPTS_MAX: u32 = 5;

/// #1485 前の挙動を再現するか
pub fn legacy_mode() -> bool {
    std::env::var(LEGACY_ENV).is_ok_and(|v| v == "1" || v == "true" || v == "on")
}

/// この環境で試す回数（env で上書き可能。検証用）
pub fn attempts_max() -> u32 {
    std::env::var(ATTEMPTS_ENV)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .map(|v| v.clamp(1, 20))
        .unwrap_or(ATTEMPTS_MAX)
}

/// `attempt` 回目（**1 始まり**）の前に待つ秒数（純関数）。
/// 表を超えた回はいちばん長い待ちを繰り返す（env で回数だけ増やしたときの形）
pub fn backoff_secs(attempt: u32) -> u64 {
    if let Some(fixed) = std::env::var(BACKOFF_SECS_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        return fixed.max(1);
    }
    backoff_secs_from(BACKOFF_SECS, attempt)
}

/// [`backoff_secs`] の判定本体（env を見ない純関数。テストはこちらを固定する）
pub fn backoff_secs_from(table: &[u64], attempt: u32) -> u64 {
    if table.is_empty() {
        return 0;
    }
    let idx = attempt.max(1) as usize - 1;
    table[idx.min(table.len() - 1)]
}

/// 「ユーザーが起動していた」の記録（`<data_dir>/remote/tako-remote.desired`）。
///
/// **ファイルが在ること自体が意図**で、中身は「いつから」だけを持つ。
/// 読めない内容でも意図は在ったとみなす（[`crate::remote::read_desired`]）ので、
/// 真偽を中身のフィールドで二重に持たない
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredState {
    /// `tako remote start` が成功した時刻（unix epoch 秒）
    #[serde(default)]
    pub since: u64,
}

/// 自動復帰をしなかった理由（#1485）。
///
/// [`autostart_decision`] の戻り値であり、**「なぜ起こさないか」を出すすべての面
/// （`tako remote status` / MCP / persist.log / ステータスバーのチップ）がこの 1 つを引く**。
/// 真偽値だけを返すと、画面ごとに理由を書き直すことになり片方だけ嘘になる（#1473 と同じ）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartSkip {
    /// 一度も `tako remote start` していない / `tako remote stop` 済み。
    /// **これが既定**（何も設定していない環境では何も起きない）
    NotDesired,
    /// daemon は既に動いている（多重起動しない）
    AlreadyRunning,
    /// この OS では daemon を起動できない（Windows は unix ソケット前提。#971）
    Unsupported,
    /// A/B で自動復帰を切っている（`TAKO_1485_LEGACY=1`）
    Disabled,
}

impl AutostartSkip {
    /// 診断・JSON 用の識別子（ASCII 固定）
    pub fn tag(self) -> &'static str {
        match self {
            Self::NotDesired => "not-desired",
            Self::AlreadyRunning => "already-running",
            Self::Unsupported => "unsupported",
            Self::Disabled => "disabled",
        }
    }

    /// 人が読む理由（CLI / チップ / persist.log が使う）
    pub fn describe(self) -> String {
        match self {
            Self::NotDesired => {
                "リモートを起動した記録が無いため何もしません（`tako remote start` で起動すると次回から自動で戻ります）"
                    .to_string()
            }
            Self::AlreadyRunning => "リモートは既に起動しています".to_string(),
            Self::Unsupported => {
                "この OS ではリモート daemon を起動できません（#971）".to_string()
            }
            Self::Disabled => {
                format!("{LEGACY_ENV}=1 で自動復帰を切っています")
            }
        }
    }

    /// 画面（ステータスバーのチップ）へ出すべき理由か。
    ///
    /// **出すのは「戻すつもりだったのに戻せなかった」ときだけ**。
    /// `NotDesired` は設定どおりの正常な動きなので、GUI を起動するたびに
    /// 「リモートは起動しません」と出すことになり邪魔にしかならない
    pub fn is_noteworthy(self) -> bool {
        matches!(self, Self::Unsupported)
    }
}

/// 自動復帰の判断材料（純粋な入力。ファイル I/O は呼ぶ側が済ませる）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutostartInput {
    /// 「起動していた」の記録が在るか
    pub desired: bool,
    /// daemon が既に生きているか
    pub daemon_running: bool,
    /// この OS で daemon を起動できるか（`platform::support` のゲート）
    pub supported: bool,
    /// A/B（`TAKO_1485_LEGACY=1`）
    pub legacy: bool,
}

/// daemon を立て直すか、立て直さないならなぜかを返す（#1485）。
///
/// 順序は「人が読んで納得する順」= A/B → OS → 意図 → 現況。
/// **`legacy` を最初に見る**のは、A/B を立てた検証で「立てたのに理由が
/// `not-desired` に化ける」のを避けるため（アームの効きが読めなくなる）
pub fn autostart_decision(input: &AutostartInput) -> Result<(), AutostartSkip> {
    if input.legacy {
        return Err(AutostartSkip::Disabled);
    }
    if !input.supported {
        return Err(AutostartSkip::Unsupported);
    }
    if !input.desired {
        return Err(AutostartSkip::NotDesired);
    }
    if input.daemon_running {
        return Err(AutostartSkip::AlreadyRunning);
    }
    Ok(())
}

/// 自動復帰が使える OS か（`platform::support` の 1 表を引く）。
///
/// **`cfg!(windows)` で判定しない**。自動復帰は `tako remote start` を自動で
/// 呼ぶだけなので、使えるかどうかは `tako_remote_start` の宣言そのもの
/// （Windows は daemon が unix ソケット前提で `Pending` = #971）。
/// 判定を別に持つと、Windows の serve が動くようになった日に片方だけ残る。
/// マトリクスへ `tako_remote_autostart` という**キーは足さない**:
/// マトリクスのキーは MCP ツール名が正で、公開していないキーを足すと
/// `platform_parity` の T2（逆被覆）が落ちる
pub fn platform_supported() -> bool {
    use tako_core::platform::support;
    support::support_for(support::Platform::current(), SUPPORT_KEY).is_some_and(|s| s.is_usable())
}

/// 自動復帰の可否を引くマトリクスのキー（= 自動で呼ぶ操作そのもの）
pub const SUPPORT_KEY: &str = "tako_remote_start";

/// 実環境の材料を集めて [`autostart_decision`] を引く（#1485）。
///
/// **判断の本体は純関数のまま**にして、ここは材料集め（ファイル・プロセス・
/// マトリクス・env）だけを持つ。GUI はこれを background で呼び、**試行のたびに
/// 引き直す**（待っている間に人が `tako remote start` / `stop` を打っているかもしれない）
pub fn probe() -> Result<(), AutostartSkip> {
    autostart_decision(&AutostartInput {
        desired: crate::remote::read_desired().is_some(),
        daemon_running: crate::remote::daemon_status()["running"].as_bool() == Some(true),
        supported: platform_supported(),
        legacy: legacy_mode(),
    })
}

/// 自動復帰の結果（`<data_dir>/remote/tako-remote.autostart`）。
///
/// GUI が書き `tako remote status`（別プロセス）が読む —— [`crate::remote_serve::ServeHealth`]
/// と同じ理由で、GUI のメモリを覗く代わりのいちばん軽い手段。**短命な稼働状態**なので
/// #916 の移行台帳には載せない（次の GUI 起動で必ず書き直され、読めなければ
/// 「記録が無い」として扱えばよいだけ）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastAutostart {
    /// 記録した時刻（unix epoch 秒）
    pub at: u64,
    /// [`RESULT_STARTED`] / [`RESULT_SKIPPED`] / [`RESULT_FAILED`]
    pub result: String,
    /// `spawn_daemon` を呼んだ回数（skipped なら 0）
    #[serde(default)]
    pub attempts: u32,
    /// 分類（[`AutostartSkip::tag`] か `spawn-failed`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 人が読む理由（daemon が返した不足項目など）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 立て直した
pub const RESULT_STARTED: &str = "started";
/// 立て直す必要が無かった / 対象外だった（理由は `reason`）
pub const RESULT_SKIPPED: &str = "skipped";
/// 立て直そうとして失敗した（理由は `detail`）
pub const RESULT_FAILED: &str = "failed";

/// 失敗の分類（`spawn_daemon` が Err を返した）
pub const REASON_SPAWN_FAILED: &str = "spawn-failed";

/// 記録へ載せる理由文の上限（文字数）。
/// daemon の stderr はセットアップの不足項目を列挙してくるので長くなりうる
pub const DETAIL_MAX_CHARS: usize = 300;

/// 理由文を記録・診断へ載せる形へ整える（純関数）。
/// 改行を 1 行へ畳み、長すぎるものは切る（`status` の JSON と persist.log の 1 行が壊れない）
pub fn trim_detail(detail: &str) -> String {
    let joined = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= DETAIL_MAX_CHARS {
        return joined;
    }
    let cut: String = joined.chars().take(DETAIL_MAX_CHARS).collect();
    format!("{cut}…")
}

impl LastAutostart {
    /// 立て直せた
    pub fn started(at: u64, attempts: u32) -> Self {
        Self {
            at,
            result: RESULT_STARTED.to_string(),
            attempts,
            reason: None,
            detail: None,
        }
    }

    /// 立て直さなかった（理由つき）
    pub fn skipped(at: u64, skip: AutostartSkip) -> Self {
        Self {
            at,
            result: RESULT_SKIPPED.to_string(),
            attempts: 0,
            reason: Some(skip.tag().to_string()),
            detail: Some(skip.describe()),
        }
    }

    /// 立て直せなかった（daemon が返した理由つき）
    pub fn failed(at: u64, attempts: u32, detail: &str) -> Self {
        Self {
            at,
            result: RESULT_FAILED.to_string(),
            attempts,
            reason: Some(REASON_SPAWN_FAILED.to_string()),
            detail: Some(trim_detail(detail)),
        }
    }

    /// 画面へ出すべき結果か（**失敗だけ**。成功と skip は通知欄に出さない）。
    /// 立て直せたことはチップが running になることで分かるので、
    /// バナーにすると GUI を起動するたびに出て邪魔になる
    pub fn is_failure(&self) -> bool {
        self.result == RESULT_FAILED
    }

    /// persist.log へ残す 1 行（**書式は 1 か所**。診断の grep が 1 通りで済む）
    pub fn log_line(&self) -> String {
        let mut line = format!(
            "リモート自動復帰: 結果={} 試行={}",
            self.result, self.attempts
        );
        if let Some(reason) = &self.reason {
            line.push_str(&format!(" 分類={reason}"));
        }
        if let Some(detail) = &self.detail {
            line.push_str(&format!(" 理由={}", trim_detail(detail)));
        }
        line
    }
}

/// 途中の試行が落ちたときの 1 行（純関数。書式は [`LastAutostart::log_line`] と揃える）。
///
/// **最後の結果だけを残すと、再試行している間は何も残らない**（既定のバックオフは
/// 合計 77 秒で、その間ユーザーにも診断にも何も見えない = #1485 が消したい「黙る」）。
/// 画面へはまだ出さない（20 秒後に消えるバナーは邪魔になるだけ）ので、
/// **診断にだけ**残す。`残り` を出すのは「まだ諦めていない」と分かるようにするため
pub fn attempt_log_line(attempt: u32, max: u32, detail: &str) -> String {
    format!(
        "リモート自動復帰: 試行={attempt}/{max} 失敗（再試行します） 理由={}",
        trim_detail(detail)
    )
}

/// 途中の試行の失敗を診断へ残す（意図が在る環境だけ）
pub fn log_attempt_failure(attempt: u32, max: u32, detail: &str) {
    if crate::remote::read_desired().is_none() {
        return;
    }
    crate::diag::persist_log(&attempt_log_line(attempt, max, detail));
}

/// いまの時刻（unix epoch 秒）。記録の時刻はこの 1 実装を通す
pub fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 結果を残す（#1485）。ファイル（`tako remote status` が読む）と persist.log の
/// **両方へ 1 回で**書く = 片方だけ残る状態を作らない。
///
/// **意図が無い環境では何も残さない**。remote を一度も使っていない人の
/// `<data_dir>/remote/` に自動復帰の記録だけが生えるのはノイズで、
/// 「何も起きていない」ことは記録の不在そのものが表している
pub fn record_outcome(last: &LastAutostart) {
    if crate::remote::read_desired().is_none() {
        return;
    }
    crate::remote::write_last_autostart(last);
    crate::diag::persist_log(&last.log_line());
}

/// `tako remote status` / MCP `tako_remote_status` に載せる形へ変換する（純関数）。
///
/// **`running` と同じ応答に必ず載せる**（daemon が止まっていても載る）のが要点:
/// 「止まっている」だけを返すと、戻すつもりが在ったのか無かったのかを
/// ユーザーも AI も区別できない = #1485 が消したい「黙る」がここに残る
pub fn status_fields(desired: Option<&DesiredState>, last: Option<&LastAutostart>) -> Value {
    let mut out = json!({ "desired": desired.is_some() });
    if let Some(d) = desired {
        if d.since > 0 {
            out["desired_since"] = json!(d.since);
        }
    }
    if let Some(l) = last {
        out["last_autostart"] = serde_json::to_value(l).unwrap_or(Value::Null);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(desired: bool, running: bool) -> AutostartInput {
        AutostartInput {
            desired,
            daemon_running: running,
            supported: true,
            legacy: false,
        }
    }

    #[test]
    fn 記録があってdaemonが居なければ立て直す() {
        assert_eq!(autostart_decision(&input(true, false)), Ok(()));
    }

    #[test]
    fn 記録が無ければ何もしない() {
        assert_eq!(
            autostart_decision(&input(false, false)),
            Err(AutostartSkip::NotDesired)
        );
    }

    #[test]
    fn 既に動いていれば起こさない() {
        assert_eq!(
            autostart_decision(&input(true, true)),
            Err(AutostartSkip::AlreadyRunning)
        );
    }

    #[test]
    fn 未対応osでは記録があっても起こさない() {
        let mut i = input(true, false);
        i.supported = false;
        assert_eq!(autostart_decision(&i), Err(AutostartSkip::Unsupported));
    }

    /// A/B は**いちばん先**に見る。`desired` が無い環境でアームを立てたときに
    /// 理由が `not-desired` へ化けると、アームが効いているのか判断できない
    #[test]
    fn legacyは他のどの理由より先に出る() {
        for (desired, running, supported) in [
            (true, false, true),
            (false, false, true),
            (true, true, true),
            (true, false, false),
        ] {
            let i = AutostartInput {
                desired,
                daemon_running: running,
                supported,
                legacy: true,
            };
            assert_eq!(
                autostart_decision(&i),
                Err(AutostartSkip::Disabled),
                "desired={desired} running={running} supported={supported}"
            );
        }
    }

    /// 理由の識別子は往復し、重複しない（診断の grep が一意に当たる）
    #[test]
    fn 理由の識別子は一意で説明を持つ() {
        let all = [
            AutostartSkip::NotDesired,
            AutostartSkip::AlreadyRunning,
            AutostartSkip::Unsupported,
            AutostartSkip::Disabled,
        ];
        let tags: std::collections::BTreeSet<&str> = all.iter().map(|s| s.tag()).collect();
        assert_eq!(tags.len(), all.len(), "識別子が重複している: {tags:?}");
        for s in all {
            assert!(!s.describe().is_empty(), "{} に説明が無い", s.tag());
            assert!(
                s.tag().chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{} は ASCII 固定にする",
                s.tag()
            );
        }
    }

    /// 通知欄へ出すのは「戻すつもりだったのに戻せなかった」ときだけ
    #[test]
    fn 正常な見送りは画面へ出さない() {
        assert!(!AutostartSkip::NotDesired.is_noteworthy());
        assert!(!AutostartSkip::AlreadyRunning.is_noteworthy());
        assert!(!AutostartSkip::Disabled.is_noteworthy());
        assert!(AutostartSkip::Unsupported.is_noteworthy());
    }

    /// 待ちの表は定数で固定する（env なしの純関数側）
    #[test]
    fn バックオフは表のとおりで表を超えたら最長を繰り返す() {
        assert_eq!(BACKOFF_SECS, &[2, 5, 10, 20, 40]);
        assert_eq!(backoff_secs_from(BACKOFF_SECS, 1), 2);
        assert_eq!(backoff_secs_from(BACKOFF_SECS, 2), 5);
        assert_eq!(backoff_secs_from(BACKOFF_SECS, 5), 40);
        assert_eq!(backoff_secs_from(BACKOFF_SECS, 9), 40);
        // 0 は 1 回目として扱う（呼ぶ側の 1 始まりを壊さない）
        assert_eq!(backoff_secs_from(BACKOFF_SECS, 0), 2);
        assert_eq!(backoff_secs_from(&[], 3), 0);
    }

    /// 最後の試行までに tailscaled の立ち上がりを跨げる長さがある
    #[test]
    fn 試行の合計は一分以上ある() {
        let total: u64 = (1..=ATTEMPTS_MAX).map(backoff_secs_from_default).sum();
        assert!(total >= 60, "合計 {total} 秒では再起動直後を跨げない");
        assert_eq!(ATTEMPTS_MAX as usize, BACKOFF_SECS.len());
    }

    fn backoff_secs_from_default(attempt: u32) -> u64 {
        backoff_secs_from(BACKOFF_SECS, attempt)
    }

    /// 可否はマトリクスの 1 マス（`tako_remote_start`）に従う。
    /// **`tako_remote_autostart` というキーは足さない**（MCP に無いキーは T2 が落とす）
    #[test]
    fn 可否はマトリクスのremote_startに従う() {
        use tako_core::platform::support::{self, Platform};
        assert_eq!(SUPPORT_KEY, "tako_remote_start");
        for platform in [Platform::MacOs, Platform::Windows] {
            let declared = support::support_for(platform, SUPPORT_KEY)
                .unwrap_or_else(|| panic!("{platform:?} に {SUPPORT_KEY} の宣言が無い"));
            assert_eq!(
                declared.is_usable(),
                platform == Platform::MacOs,
                "{platform:?}: 宣言（{}）と自動復帰の可否がずれている",
                declared.status()
            );
        }
        assert!(
            support::support_for(Platform::MacOs, "tako_remote_autostart").is_none(),
            "マトリクスのキーは MCP ツール名が正（T2 が落ちる）"
        );
        assert_eq!(platform_supported(), cfg!(target_os = "macos"));
    }

    #[test]
    fn 理由文は一行へ畳んで切る() {
        assert_eq!(trim_detail("a\nb  c"), "a b c");
        let long = "あ".repeat(DETAIL_MAX_CHARS + 50);
        let cut = trim_detail(&long);
        assert_eq!(cut.chars().count(), DETAIL_MAX_CHARS + 1, "末尾の … を含む");
        assert!(cut.ends_with('…'));
    }

    /// status は daemon が止まっていても意図を名乗る（黙らない）
    #[test]
    fn statusは意図と直近の結果を載せる() {
        let none = status_fields(None, None);
        assert_eq!(none["desired"], json!(false));
        assert!(none.get("last_autostart").is_none());

        let desired = DesiredState {
            since: 1_700_000_000,
        };
        let last = LastAutostart::failed(1_700_000_100, 5, "Tailscale が\n見つかりません");
        let v = status_fields(Some(&desired), Some(&last));
        assert_eq!(v["desired"], json!(true));
        assert_eq!(v["desired_since"], json!(1_700_000_000));
        assert_eq!(v["last_autostart"]["result"], json!(RESULT_FAILED));
        assert_eq!(v["last_autostart"]["attempts"], json!(5));
        assert_eq!(v["last_autostart"]["reason"], json!(REASON_SPAWN_FAILED));
        assert_eq!(
            v["last_autostart"]["detail"],
            json!("Tailscale が 見つかりません")
        );
    }

    /// 記録は JSON を往復する（別プロセスの `tako remote status` が読む）
    #[test]
    fn 記録はjsonを往復する() {
        for last in [
            LastAutostart::started(10, 2),
            LastAutostart::skipped(11, AutostartSkip::NotDesired),
            LastAutostart::failed(12, 5, "理由"),
        ] {
            let text = serde_json::to_string(&last).expect("直列化できる");
            let back: LastAutostart = serde_json::from_str(&text).expect("復元できる");
            assert_eq!(back, last);
        }
    }

    /// 旧世代の記録（フィールドが少ない）でも読める = 移行を持たなくてよい根拠
    #[test]
    fn 記録は最小のjsonからも読める() {
        let back: LastAutostart =
            serde_json::from_str(r#"{"at":1,"result":"started"}"#).expect("既定で埋まる");
        assert_eq!(back.attempts, 0);
        assert!(back.reason.is_none());
    }

    #[test]
    fn 意図の記録は空のjsonからも読める() {
        let back: DesiredState = serde_json::from_str("{}").expect("既定で埋まる");
        assert_eq!(back.since, 0);
    }

    /// persist.log の 1 行は分類と理由を必ず持つ（無言の行を作らない）
    #[test]
    fn 診断の一行は分類と理由を持つ() {
        let line = LastAutostart::failed(1, 3, "Tailscale 未セットアップ").log_line();
        assert!(line.contains("結果=failed"), "{line}");
        assert!(line.contains("試行=3"), "{line}");
        assert!(line.contains("分類=spawn-failed"), "{line}");
        assert!(line.contains("理由=Tailscale 未セットアップ"), "{line}");
        assert!(!line.contains('\n'), "1 行に収める: {line}");

        let ok = LastAutostart::started(1, 1).log_line();
        assert!(ok.contains("結果=started"), "{ok}");
    }
}

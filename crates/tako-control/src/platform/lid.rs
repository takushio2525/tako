//! 蓋を閉じたまま走らせ続ける制御（抽象境界 B9 のうち蓋ぶん）。#697 / #2857
//!
//! 手段は OS で違う。呼び手の `sleep_guard` は下の「OS をまたぐ口」（[`control_supported`] /
//! [`apply_override`] / [`override_active`] ほか）だけを呼び、OS を意識しない。`cfg` はこのファイル
//! （と [`pmset`]）の内側に閉じている。
//!
//! | OS | 手段 | 権限 | 倒した記録 |
//! |---|---|---|---|
//! | macOS | clamshell 検知 + sudoers + `pmset disablesleep`（[`pmset`]。#218。#2857 で `sleep_guard.rs` から移した） | 初回に管理者パスワード | 所有者つきで持つ（#2904。`lid-guard.json` の `method: pmset`。いまの値は IORegistry から読む） |
//! | Windows | 電源プランの lid action（このファイルの本体 = 電源プランの腕。#697） | 不要 | 所有者つきで持つ（#1373） |
//!
//! どちらの腕も**自分が倒した上書きだけ**を戻す（設定 off・エージェントの一段落・正常終了）。
//! 持ち主の死んだ記録は、macOS の腕は起動時の残留チェックと同じ門（隔離でない・他の tako が居ない）を
//! 通ったときだけ戻す（記録が元値を持たず、いまの 1 が別の data dir の tako のものでないと言えないため。
//! Windows の腕は #1373 のまま門を通さず、元値と `should_restore` で戻す）。
//! | その他 | 持たない | — | — |
//!
//! 以下の節は電源プランの腕（Windows）の説明。[`supported`] / [`is_active`] / [`set_stay_awake`] /
//! [`clear_residual`] はこの腕の口で、macOS では使わない（[`supported`] が false）。
//!
//! ## 何を倒すのか
//!
//! Windows で蓋を閉じたときの動作は電源プランの設定 `GUID_LIDCLOSE_ACTION`
//! （サブグループ `SUB_BUTTONS`）で決まる。値は
//! `0 = 何もしない` / `1 = スリープ` / `2 = 休止状態` / `3 = シャットダウン`。
//! これを一時的に 0 へ倒し、解除時に元値へ戻す
//! （macOS の `pmset disablesleep 1/0` と同じ「永続設定を一時的に倒す」形）。
//!
//! **管理者権限は要らない**。実測（Windows 11 Home・非管理者）で
//! `PowerWriteACValueIndex` が `ERROR_SUCCESS` を返し、読み戻し・復元まで通ることを確認した。
//! macOS 側の sudoers 登録に相当する初回セットアップは不要。
//!
//! ## なぜ電源要求（`PowerSetRequest`）では足りないのか
//!
//! 蓋を閉じたまま走らせ続けるには**引き金の違う 2 つ**を止める必要がある。
//!
//! | 引き金 | 止める手段 | どこ |
//! |---|---|---|
//! | アイドル（無操作が続く） | `PowerSetRequest(SystemRequired)` | `platform::power`（#524） |
//! | 蓋を閉じる | `LIDACTION = 0` | ここ（#697） |
//!
//! 電源要求は蓋の動作には一切効かない。片方だけでは、この実機のような
//! Modern Standby（S0 低電力アイドル）機で蓋を閉じるとプランどおりスリープして処理が止まる。
//!
//! ## 残留対策（**最重要**）
//!
//! 上書きは電源プランに書かれる永続設定なので、倒したまま tako が死ぬと
//! **ユーザーの PC が蓋を閉じてもスリープしないまま**になる（鞄の中で電池が尽きる）。
//! そこで倒す**前に**元値をディスクへ保存し、次回起動時に残っていれば戻す
//! （macOS の `check_disablesleep_residual` と同じ役割）。
//! ユーザーが自分で設定を変えていた場合は上書きしない（`should_restore` 参照）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
// プロセスの生死と起動時刻の語彙。**テスト専用の型ではない**（`tako test-residue` の
// 所有者判定と同じ 1 実装を使い、「生きていないと言い切れるときだけ `Dead`」の
// 意味をこちらでもズラさないため。#1296 / #1373）
use tako_core::test_residue::Owner;

/// `GUID_LIDCLOSE_ACTION` の「何もしない」
pub const LID_ACTION_DO_NOTHING: u32 = 0;

/// 電源レール。Windows の電源プランは AC / DC で別々の値を持つ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rail {
    /// AC 電源接続時
    Ac,
    /// バッテリー駆動時
    Dc,
}

/// 倒す対象のレールを決める。
///
/// `sleep_guard` の `PowerCondition` と意味を揃える:
/// `ac-only` は AC のみ、`always` は AC + DC。
///
/// **バッテリー側を既定で触らない**のが安全側の設計。鞄の中で蓋を閉じるのは
/// たいていバッテリー駆動なので、既定（`ac-only`）のままなら
/// 上書きが残留しても電池が尽きる事故にはならない
pub fn rails_for(include_battery: bool) -> &'static [Rail] {
    if include_battery {
        &[Rail::Ac, Rail::Dc]
    } else {
        &[Rail::Ac]
    }
}

/// 記録を書いた tako プロセス（#1373）。
///
/// `lid-guard.json` は `data_dir` に 1 つで、**複数の tako-app プロセスが共有する**
/// （隔離されるのは `TAKO_ISOLATED` / `TAKO_DATA_DIR` を立てたときだけ）。
/// 所有者を書いておかないと、busy なインスタンス A が倒した上書きを、idle な B の
/// tick が「自分の記録」と思って戻してしまう（macOS 側で #449 として直した事故と同型。
/// A は自分の写しを信じて倒し直さないので、**画面は「有効」のまま実機は眠る**）。
///
/// pid だけでは**pid の再利用**を見分けられないので起動時刻と対にする
/// （材料は `procinfo::start_time_unix`。#1282 / #1296 と同じ作り）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordOwner {
    /// 記録を書いたプロセスの pid
    pub pid: u32,
    /// そのプロセスの起動時刻（UNIX 秒）。取得手段が無い OS では `None`
    #[serde(default)]
    pub started_unix: Option<u64>,
}

impl RecordOwner {
    /// いま走っているこのプロセス
    pub fn current() -> Self {
        let pid = std::process::id();
        Self {
            pid,
            started_unix: tako_core::platform::procinfo::start_time_unix(pid),
        }
    }

    /// 同じプロセスを指しているか。
    ///
    /// 起動時刻は**両方取れているときだけ**比べる。取得手段の無い OS で
    /// 「取れないから別人」へ倒すと、自分が書いた記録すら戻せなくなる
    pub fn is_same(&self, other: &Self) -> bool {
        self.pid == other.pid
            && match (self.started_unix, other.started_unix) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }
}

/// 記録がどちらの手段で倒したものか（#2904）。
///
/// `lid-guard.json` は data dir に 1 つで、OS の腕どうしで共有する。#2904 より前は電源プランの腕
/// （Windows）だけが書いていたので、欄の無い記録は [`LidMethod::PowerPlan`] として読む
/// （`serde(default)` = 旧い記録がそのまま読めるので移行の Step は要らない = #916）。
/// 既定の値は**書き出さない**ので、電源プランの腕が書く記録は #2904 の前後でバイト単位で同じ
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LidMethod {
    /// 電源プランの lid action（Windows。#697 / #1373）
    #[default]
    PowerPlan,
    /// `pmset disablesleep`（macOS。#218 / #2904）。元値は持たない（戻す先は常に `0`）
    Pmset,
}

impl LidMethod {
    /// 既定（電源プランの腕）か。記録へ書き出さない判定に使う
    pub fn is_power_plan(&self) -> bool {
        *self == Self::PowerPlan
    }
}

/// 倒す前に保存しておく元の状態。クラッシュしてもここから戻せる
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedLidState {
    /// 倒した手段（#2904）。欄の無い記録は電源プランの腕（[`LidMethod::PowerPlan`]）
    #[serde(default, skip_serializing_if = "LidMethod::is_power_plan")]
    pub method: LidMethod,
    /// 倒した電源プランの GUID（文字列）。
    /// ユーザーがプランを切り替えても、書いたプランへ戻せるように持つ。
    /// `pmset` の記録は手段名（[`pmset::RECORD_SCHEME`]）を入れる（GUID ではない）
    pub scheme: String,
    /// AC 側の元値。倒していなければ `None`
    pub ac: Option<u32>,
    /// DC 側の元値。倒していなければ `None`
    pub dc: Option<u32>,
    /// 記録を書いたプロセス（#1373）。
    ///
    /// **`None` は #1373 より前に書かれた記録**（所有者を持たない形式）。
    /// `serde(default)` で旧い `lid-guard.json` がそのまま読めるので移行手順は要らない
    #[serde(default)]
    pub owner: Option<RecordOwner>,
}

impl SavedLidState {
    /// 記録されているレールと元値の組
    pub fn entries(&self) -> Vec<(Rail, u32)> {
        let mut out = Vec::new();
        if let Some(v) = self.ac {
            out.push((Rail::Ac, v));
        }
        if let Some(v) = self.dc {
            out.push((Rail::Dc, v));
        }
        out
    }

    /// このレールを倒したか
    pub fn covers(&self, rail: Rail) -> bool {
        match rail {
            Rail::Ac => self.ac.is_some(),
            Rail::Dc => self.dc.is_some(),
        }
    }

    /// 倒したいレールと**過不足なく**一致しているか。
    ///
    /// 「足りているか」（`wanted` を全部覆っているか）では**不十分**。
    /// 電源条件を `always` → `ac-only` へ変えたとき、覆えてはいるので何もせず
    /// **DC を倒しっぱなしにしてしまう**（バッテリー駆動で蓋を閉じてもスリープしない機械が残る）
    pub fn covers_exactly(&self, wanted: &[Rail]) -> bool {
        [Rail::Ac, Rail::Dc]
            .iter()
            .all(|r| self.covers(*r) == wanted.contains(r))
    }
}

/// 元値へ戻してよいかを判定する純粋関数。
///
/// 現在値が我々の書いた `0`（何もしない）のままなら戻す。それ以外なら
/// **ユーザーが自分で設定を変えた**ということなので触らない
/// （倒したあとに「蓋を閉じたら休止状態」へ変えた人の設定を、
/// tako の解除で勝手に戻してしまうのを防ぐ）
pub fn should_restore(current: u32, saved_original: u32) -> bool {
    // 元値がもともと 0 なら戻しても変わらない（無害な no-op）
    current == LID_ACTION_DO_NOTHING && saved_original != LID_ACTION_DO_NOTHING
}

/// 起動時の残留解除を行うべきかを判定する純粋関数（macOS 側 `should_clear_residual` と同じ方針）。
/// `Ok(())` なら解除すべき、`Err(理由)` ならスキップ
pub fn should_clear_residual(
    is_isolated: bool,
    other_instance_running: bool,
    saved_exists: bool,
) -> Result<(), &'static str> {
    if is_isolated {
        return Err("隔離モード（TAKO_ISOLATED）のためスキップ");
    }
    if other_instance_running {
        // 別の tako が倒している最中かもしれない。奪って戻すと相手の機能が壊れる
        return Err("他の tako プロセスが動作中のためスキップ");
    }
    if !saved_exists {
        return Err("上書きの記録なし（残留なし）");
    }
    Ok(())
}

/// 残留記録の置き場所。`TAKO_DATA_DIR` で隔離できる（#177）
fn state_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("lid-guard.json"))
}

/// 記録に対する、このプロセスの立場（#1373）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordClaim {
    /// 自分が書いた記録
    Mine,
    /// 所有者を持たない記録（#1373 より前の形式）
    Unowned,
    /// 所有者がもう生きていない（前回の異常終了、または pid の再利用）
    Abandoned,
    /// **他の生きている tako が保持している**。触ってはいけない
    Foreign {
        /// 保持しているプロセスの pid（診断に出す）
        pid: u32,
    },
}

/// この記録へ手を出してよいか。`Foreign` のときだけ false
pub fn may_touch(claim: RecordClaim) -> bool {
    !matches!(claim, RecordClaim::Foreign { .. })
}

/// 記録の状態と要求から決まる次の操作（#1373）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LidAction {
    /// 何もしない
    Nothing,
    /// 記録の元値へ戻して記録を捨てる
    Restore,
    /// いったん元値へ戻してから、改めて倒し直す
    /// （電源条件が変わった / 所有者の居ない記録を引き取る）
    RestoreThenAcquire,
    /// 倒す（記録が無い状態から）
    Acquire,
}

/// `SystemTime` を UNIX 秒へ。比較の粒度を記録側（UNIX 秒）へ揃えるためだけに使う
fn unix_secs(t: std::time::SystemTime) -> Option<u64> {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// 記録の所有者を判定する純粋関数（#1373）。
///
/// `probe` は「その pid のプロセスの状態」を引く関数で、OS 依存をここから追い出すために
/// **引数で受ける**（Windows 実機が無くても 4 通りすべてを macOS の CI で固定できる）。
/// 判定に迷ったら [`RecordClaim::Foreign`]（触らない）側へ倒す
pub fn claim_for(
    owner: Option<&RecordOwner>,
    me: &RecordOwner,
    probe: impl Fn(u32) -> Owner,
) -> RecordClaim {
    let Some(owner) = owner else {
        return RecordClaim::Unowned;
    };
    if owner.is_same(me) {
        return RecordClaim::Mine;
    }
    match probe(owner.pid) {
        Owner::Dead => RecordClaim::Abandoned,
        // 生死を判定できない = 奪わない
        Owner::Unknown => RecordClaim::Foreign { pid: owner.pid },
        Owner::Alive { started } => {
            // pid は生きているが、そのプロセスは記録を書いた本人ではない = pid の再利用。
            // 比べる 2 つはどちらも `start_time_unix` が出した秒なので丸めの余裕は要らない
            // （`test_residue::judge` が `REUSE_SLACK` を持つのは、比べる相手が
            // ファイルシステムの作成時刻で粒度が違うため）
            let recycled = match (owner.started_unix, started.and_then(unix_secs)) {
                (Some(recorded), Some(live)) => recorded != live,
                _ => false,
            };
            if recycled {
                RecordClaim::Abandoned
            } else {
                RecordClaim::Foreign { pid: owner.pid }
            }
        }
    }
}

/// 記録の状態と要求から次の操作を決める純粋関数（#1373）。
///
/// 電源プランを実際に触る側（[`run_action`]）はこの結果に従うだけにして、
/// **「誰の記録か」の判定が 1 か所に留まる**ようにする。
/// `saved` は「記録」と「それに対する立場」の組で、記録が無ければ `None`
pub fn decide(
    enable: bool,
    wanted: &[Rail],
    saved: Option<(&SavedLidState, RecordClaim)>,
) -> LidAction {
    let Some((saved, claim)) = saved else {
        // 記録が無い。倒すときだけ書く
        return if enable {
            LidAction::Acquire
        } else {
            LidAction::Nothing
        };
    };
    if !may_touch(claim) {
        // 他の生きた tako が保持している。**倒す側も解除側も触らない**
        // （倒したいなら、相手が解除した次の tick で自分が取り直す）
        return LidAction::Nothing;
    }
    if !enable {
        return LidAction::Restore;
    }
    if claim == RecordClaim::Mine && saved.covers_exactly(wanted) {
        return LidAction::Nothing; // 自分が過不足なく倒している
    }
    // 電源条件が変わった（ac-only ⇔ always）か、所有者の居ない記録を引き取る。
    // 引き取りは**必ず「戻してから倒し直す」**。倒れたままの現在値を元値として
    // 記録し直すと、ユーザーの蓋設定が永久に失われる（#1373 の症状 2）
    LidAction::RestoreThenAcquire
}

/// 記録の所有者の状態を実際に引く。
///
/// 一括走査用の [`tako_core::test_residue::OwnerProbe`] は Windows で在籍列挙を
/// 1 回だけ取る作りなので、1 件を 2 秒ごとに引くここでは使わない
/// （列挙が UI スレッドの定期 I/O になる = #212 / #168）。
/// 語彙（[`Owner`]）は共有して「生きていないと言い切れるときだけ `Dead`」の意味をズラさない
fn probe_owner(pid: u32) -> Owner {
    if pid == 0 || pid > i32::MAX as u32 {
        // pid として使われない値 = 記録が壊れている。触らない
        return Owner::Unknown;
    }
    // 起動時刻は**走っているものだけ**を採る（#2284）。Windows は終わったプロセスでも
    // オブジェクトが残る間は起動時刻が読めるので、取れたことを生存の証拠にすると
    // `pid_alive`（終了コードを先に問う = #2275）を素通りして死んだ所有者を `Alive` と読む
    if let Some(secs) = tako_core::platform::process::live_start_time_unix(pid) {
        return Owner::Alive {
            started: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
        };
    }
    // 起動時刻が取れない = 居ない・終わった、または読めない。生きているなら起動時刻なしの
    // `Alive` へ倒す（`claim_for` はそれを `Foreign` = 触らない と読む）
    if tako_core::platform::process::pid_alive(pid) {
        Owner::Alive { started: None }
    } else {
        Owner::Dead
    }
}

/// このプロセスの身元。走っている間は変わらないので 1 度だけ引く
fn me() -> &'static RecordOwner {
    static ME: std::sync::OnceLock<RecordOwner> = std::sync::OnceLock::new();
    ME.get_or_init(RecordOwner::current)
}

/// 記録と、それに対するこのプロセスの立場を組にする
fn with_claim(saved: Option<&SavedLidState>) -> Option<(&SavedLidState, RecordClaim)> {
    let saved = saved?;
    let claim = claim_for(saved.owner.as_ref(), me(), probe_owner);
    Some((saved, claim))
}

/// 蓋閉じ継続の上書き・復元の断り（#2365）。`error` / `detail` は OS のエラー文（言語に依らず
/// そのまま載せる）、パスは表記のまま、`rc` は Win32 の戻り値。
///
/// 日英の対は [`Self::text_in`] の (バリアント, 言語) の網羅 match の 1 か所に置く。Windows の
/// 電源プランの文もここ（実装の `cfg` の外）に置くので、macOS の単体テストで日英を検査できる。
/// 従来の口（[`set_stay_awake`] / [`clear_residual`]）は `String` を返し、文は**日本語のまま**
/// （周期の更新と起動時の残留復元は診断ログへ流すだけ）。断りを表示言語で返す dispatch の
/// 蓋閉じ継続の解除（`sleep_guard::teardown_lid_control_typed`）だけが型の口を通す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LidError {
    /// 記録を読めない（権限など）
    RecordRead { path: String, error: String },
    /// 記録を解釈できない（`quarantined` は退避先。写せなければ `None`）
    RecordMalformed {
        path: String,
        reason: tako_core::load_issue::JsonReason,
        quarantined: Option<String>,
    },
    /// データディレクトリが解決できない
    DataDirUnresolved,
    /// 戻す記録が無い
    NothingToRestore,
    /// 記録の電源プランの GUID を解釈できない（値は記録のまま）
    BadSchemeGuid(String),
    /// 記録のロックを取れない（文は `config_io::LockError` の表）
    Lock(crate::config_io::LockError),
    /// 記録を書けない（文は `config_io::WriteError` の表）
    Write(crate::config_io::WriteError),
    /// 記録を JSON にできない（serde の文言のまま）
    Serialize(String),
    /// Windows: 現在の電源プランを引けない
    ActiveScheme(u32),
    /// Windows: 蓋の設定を読めない
    ReadSetting { rail: Rail, rc: u32 },
    /// Windows: 蓋の設定を書けない（5 = ERROR_ACCESS_DENIED = グループポリシーで固定）
    WriteSetting { rail: Rail, rc: u32 },
    /// Windows: 電源プランを適用できない
    ApplyScheme(u32),
    /// この OS では蓋の設定を扱えない
    Unsupported,
}

impl LidError {
    /// 表示言語を明示しての文言。(バリアント, 言語) の網羅 match なので、バリアントを足すと
    /// 日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    /// （`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）
    pub fn text_in(&self, lang: tako_core::i18n::Lang) -> String {
        use tako_core::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::RecordRead { path, error }, Ja) => {
                format!("記録を読めません（{path}）: {error}")
            }
            (Self::RecordRead { path, error }, En) => {
                format!("cannot read the record ({path}): {error}")
            }
            (
                Self::RecordMalformed {
                    path,
                    reason,
                    quarantined,
                },
                Ja,
            ) => {
                let where_to = match quarantined {
                    Some(dest) => format!("。内容は {dest} へ退避しました"),
                    None => String::new(),
                };
                format!(
                    "蓋閉じ継続の記録を解釈できません（{path}）: {}{where_to}",
                    reason.text_in(lang)
                )
            }
            (
                Self::RecordMalformed {
                    path,
                    reason,
                    quarantined,
                },
                En,
            ) => {
                let where_to = match quarantined {
                    Some(dest) => format!("; its contents were saved to {dest}"),
                    None => String::new(),
                };
                format!(
                    "cannot parse the lid-close record ({path}): {}{where_to}",
                    reason.text_in(lang)
                )
            }
            (Self::DataDirUnresolved, Ja) => "データディレクトリが解決できません".to_string(),
            (Self::DataDirUnresolved, En) => "cannot resolve the data directory".to_string(),
            (Self::NothingToRestore, Ja) => "戻す記録がありません".to_string(),
            (Self::NothingToRestore, En) => "there is no record to restore".to_string(),
            (Self::BadSchemeGuid(scheme), Ja) => {
                format!("記録の GUID を解釈できません: {scheme}")
            }
            (Self::BadSchemeGuid(scheme), En) => {
                format!("cannot parse the power plan GUID in the record: {scheme}")
            }
            (Self::Lock(e), _) => e.text_in(lang),
            (Self::Write(e), _) => e.text_in(lang),
            // 日本語は #2365 より前と同じ serde の文言のまま（英語は何の失敗かを添える）
            (Self::Serialize(e), Ja) => e.clone(),
            (Self::Serialize(e), En) => format!("cannot serialize the record: {e}"),
            (Self::ActiveScheme(rc), Ja) => format!("PowerGetActiveScheme に失敗（rc={rc}）"),
            (Self::ActiveScheme(rc), En) => format!("PowerGetActiveScheme failed (rc={rc})"),
            (Self::ReadSetting { rail, rc }, Ja) => {
                format!("蓋の設定を読めません（{rail:?}, rc={rc}）")
            }
            (Self::ReadSetting { rail, rc }, En) => {
                format!("cannot read the lid setting ({rail:?}, rc={rc})")
            }
            (Self::WriteSetting { rail, rc }, Ja) => {
                format!("蓋の設定を書けません（{rail:?}, rc={rc}）")
            }
            (Self::WriteSetting { rail, rc }, En) => {
                format!("cannot write the lid setting ({rail:?}, rc={rc})")
            }
            (Self::ApplyScheme(rc), Ja) => format!("電源プランを適用できません（rc={rc}）"),
            (Self::ApplyScheme(rc), En) => format!("cannot apply the power plan (rc={rc})"),
            (Self::Unsupported, Ja) => "この OS では蓋の設定を扱えません".to_string(),
            (Self::Unsupported, En) => "this OS has no lid setting to change".to_string(),
        }
    }

    /// 従来の `String` の口が返す文（日本語）
    pub fn into_ja(self) -> String {
        self.text_in(tako_core::i18n::Lang::Ja)
    }
}

impl std::fmt::Display for LidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for LidError {}

/// 記録の読み取り結果。`Err` = 解釈できない。
/// **「記録なし」へ丸めない**（丸めると #1373 の症状 2 が起きる）
type RecordRead = Result<Option<SavedLidState>, LidError>;

/// 記録のメモリ上の写し。外側の `None` = まだディスクから読んでいない。
///
/// **毎 tick のディスク読みを避けるため**にキャッシュする。`update()` は 2 秒ごとに
/// UI スレッドから呼ばれるので、ここで無条件にファイルを読むと
/// #212（pmset）・#168（claude agents）と同じ「UI スレッドの定期 I/O」を作ってしまう。
///
/// **写しが正でいられるのは自分が所有者のときだけ**（#1373）。記録は他プロセスと
/// 共有するので、書くと決まったら [`with_record`] がロックの下で読み直す
static CACHE: Mutex<Option<RecordRead>> = Mutex::new(None);

fn lock_cache() -> std::sync::MutexGuard<'static, Option<RecordRead>> {
    match CACHE.lock() {
        Ok(g) => g,
        // 毒されていても蓋の制御は続けたい（残留を放置する方が害が大きい）
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 記録の読み書きは**置き場所を引数で受ける**版を正にする。
///
/// `state_path()` は `TAKO_DATA_DIR` を読むプロセスグローバルなので、単体テストが
/// そこを差し替えると同一バイナリで並列に走る他のテストを巻き込む（言語グローバルで
/// 同じ事故を起こした #608 / #807 と同型。規約は `.agent/conventions.md`）。
/// 置き場所を引数にしておけば、テストは環境変数に触らずキャッシュ経路まで検査できる。
///
/// **読めない内容を「記録なし」へ丸めない**（#1373）。丸めると #169 と同じ三段連鎖で
/// 「倒したあとの 0」を元値として記録し直し、ユーザーの蓋設定が永久に失われる。
/// 解釈できない内容は `<name>.unreadable.bak` へ**写して**（#916 の作法。
/// 元のファイルは触らない）エラーを返し、倒し直しを止める
fn read_from(path: &std::path::Path) -> RecordRead {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LidError::RecordRead {
                path: path.display().to_string(),
                error: e.to_string(),
            })
        }
    };
    match serde_json::from_str::<SavedLidState>(&text) {
        Ok(state) => Ok(Some(state)),
        Err(e) => {
            let quarantine =
                tako_core::migration::quarantine_unreadable(path, &tako_core::migration::FsIo);
            Err(LidError::RecordMalformed {
                path: path.display().to_string(),
                // 種類と位置だけ（中身を引用しない = #2230）。文は包みと同じ言語で引く
                reason: tako_core::load_issue::JsonReason::of(&e),
                quarantined: quarantine.map(|dest| dest.path.display().to_string()),
            })
        }
    }
}

/// 記録を取り出す（初回だけディスクを読む）
fn load_saved() -> RecordRead {
    load_saved_from(state_path().as_deref())
}

fn load_saved_from(path: Option<&std::path::Path>) -> RecordRead {
    let mut cache = lock_cache();
    if cache.is_none() {
        *cache = Some(path.map_or(Ok(None), read_from));
    }
    cache.as_ref().expect("直前に埋めた").clone()
}

/// 記録の read-modify-write を**プロセス間で直列化**する（#1373）。
///
/// `config_io` の `mutate` 系と同じ形: ロックを取ってから**ディスクを読み直し**、
/// その下で書く。ロックファイルは `<path>.lock` で、**書くと決まってからしか取らない**
/// （無条件に取ると空の `.lock` が増える = `conventions.md`
/// 「排他ロックは「書くと決まってから」取る」）
fn with_record<T>(
    path: &std::path::Path,
    f: impl FnOnce(Option<SavedLidState>) -> Result<T, LidError>,
) -> Result<T, LidError> {
    let _lock = crate::config_io::lock_exclusive_typed(path).map_err(LidError::Lock)?;
    let fresh = read_from(path);
    *lock_cache() = Some(fresh.clone());
    f(fresh?)
}

fn store_saved(state: &SavedLidState) -> Result<(), LidError> {
    let path = state_path().ok_or(LidError::DataDirUnresolved)?;
    store_saved_at(&path, state)
}

/// 記録を書く。**原子書き込み**（tmp + fsync + rename）を通すので、並行プロセスの
/// 読み手には旧内容か新内容しか見えない（#169 の窓をここにも作らない = #1373 の症状 2）
fn store_saved_at(path: &std::path::Path, state: &SavedLidState) -> Result<(), LidError> {
    let text =
        serde_json::to_string_pretty(state).map_err(|e| LidError::Serialize(e.to_string()))?;
    crate::config_io::atomic_write_typed(path, &text).map_err(LidError::Write)?;
    *lock_cache() = Some(Ok(Some(state.clone())));
    Ok(())
}

fn clear_saved() {
    clear_saved_at(state_path().as_deref());
}

fn clear_saved_at(path: Option<&std::path::Path>) {
    if let Some(path) = path {
        let _ = std::fs::remove_file(path);
    }
    *lock_cache() = Some(Ok(None));
}

/// キャッシュを捨てて次回ディスクから読み直させる（テストと、macOS の腕の終了時 = 1 回きりの判定で
/// 写しを信じない。#2904）
#[cfg(any(target_os = "macos", test))]
fn invalidate_cache() {
    *lock_cache() = None;
}

/// 電源プランの腕（Windows）で蓋閉じ継続を制御できるか。
///
/// macOS の `pmset` の手段を含めて「この OS で制御できるか」を問うのは [`control_supported`]
pub fn supported() -> bool {
    imp::SUPPORTED
}

/// いま上書きを保持しているか（記録が残っていれば保持中）。
///
/// `status()` / `update()` から毎 tick 引かれるので、記録の複製を作らずに真偽だけ見る。
/// **読めない記録は「保持中」側へ倒す**（倒したままかもしれないものを「解除済み」と
/// 表示しない。同じエラーは [`set_stay_awake`] が理由つきで返すので UI にも出る）
pub fn is_active() -> bool {
    if !supported() {
        return false;
    }
    match load_saved() {
        Ok(saved) => saved.is_some(),
        Err(_) => true,
    }
}

/// 蓋閉じ時の動作を倒す / 元へ戻す。
///
/// - `enable = true`: `include_battery` が示すレールを「何もしない」へ倒す
/// - `enable = false`: 記録してある元値へ戻す
///
/// 同じ状態への再要求は何もしない（毎 tick 呼ばれる前提）。
/// **他の tako が保持している記録には触らない**（#1373）
pub fn set_stay_awake(enable: bool, include_battery: bool) -> Result<bool, String> {
    set_stay_awake_typed(enable, include_battery).map_err(LidError::into_ja)
}

/// [`set_stay_awake`] の型の口（#2365。dispatch の蓋閉じ継続の解除が表示言語で断る）
pub fn set_stay_awake_typed(enable: bool, include_battery: bool) -> Result<bool, LidError> {
    if !supported() {
        return Ok(false);
    }
    let wanted = rails_for(include_battery);
    // 1) まず写しだけで判定する。書かないと分かればロックもディスクも触らない
    let cached = load_saved()?;
    if decide(enable, wanted, with_claim(cached.as_ref())) == LidAction::Nothing {
        return Ok(false);
    }
    // 2) 書くと決まった。ロックの下で読み直し、同じ判定をやり直してから実行する
    let path = state_path().ok_or(LidError::DataDirUnresolved)?;
    with_record(&path, |fresh| {
        let action = decide(enable, wanted, with_claim(fresh.as_ref()));
        run_action(action, fresh.as_ref(), wanted)
    })
}

/// [`decide`] が返した操作を実行する。記録の解釈はここでは行わない
fn run_action(
    action: LidAction,
    saved: Option<&SavedLidState>,
    wanted: &[Rail],
) -> Result<bool, LidError> {
    match action {
        LidAction::Nothing => Ok(false),
        LidAction::Restore => {
            let saved = saved.ok_or(LidError::NothingToRestore)?;
            restore(saved).map(|()| true)
        }
        LidAction::RestoreThenAcquire => {
            if let Some(saved) = saved {
                restore(saved)?;
            }
            acquire(wanted).map(|()| true)
        }
        LidAction::Acquire => acquire(wanted).map(|()| true),
    }
}

/// 起動時の残留復元。戻したら説明文を返す
pub fn clear_residual(
    is_isolated: bool,
    other_instance_running: bool,
) -> Result<Option<String>, String> {
    try_clear_residual(is_isolated, other_instance_running).map_err(LidError::into_ja)
}

/// [`clear_residual`] の本体（断りは型。起動時の診断ログへ日本語で流す）
fn try_clear_residual(
    is_isolated: bool,
    other_instance_running: bool,
) -> Result<Option<String>, LidError> {
    if !supported() {
        return Ok(None);
    }
    let saved = load_saved()?;
    if should_clear_residual(is_isolated, other_instance_running, saved.is_some()).is_err() {
        return Ok(None);
    }
    // 所有者が生きているなら残留ではない（#1373）。`other_instance_running` は
    // 「tako が他にも居るか」までしか見ないので、記録の所有者そのもので判定し直す
    if with_claim(saved.as_ref()).is_some_and(|(_, claim)| !may_touch(claim)) {
        return Ok(None);
    }
    let path = state_path().ok_or(LidError::DataDirUnresolved)?;
    with_record(&path, |fresh| {
        let Some(fresh) = fresh else {
            return Ok(None); // 待っている間に他プロセスが片付けた
        };
        if !may_touch(claim_for(fresh.owner.as_ref(), me(), probe_owner)) {
            return Ok(None);
        }
        let scheme = fresh.scheme.clone();
        restore(&fresh)?;
        Ok(Some(format!(
            "蓋閉じ継続の上書きを解除しました（前回のクラッシュまたは異常終了）: scheme={scheme}"
        )))
    })
}

/// 倒す。元値を**保存してから**書く（保存前に落ちても残留しない順序）
fn acquire(rails: &[Rail]) -> Result<(), LidError> {
    let scheme = imp::active_scheme()?;
    let mut state = SavedLidState {
        method: LidMethod::PowerPlan,
        scheme: imp::guid_to_string(&scheme),
        ac: None,
        dc: None,
        // 誰が倒したかを記録に持たせる（#1373）。これが無いと別インスタンスが
        // 自分の記録と取り違えて解除してしまう
        owner: Some(me().clone()),
    };
    for rail in rails {
        let current = imp::read(&scheme, *rail)?;
        match rail {
            Rail::Ac => state.ac = Some(current),
            Rail::Dc => state.dc = Some(current),
        }
    }
    // 「書いたのに記録が無い」状態を作らないため、記録を先に置く
    store_saved(&state)?;
    for rail in rails {
        imp::write(&scheme, *rail, LID_ACTION_DO_NOTHING)?;
    }
    imp::apply(&scheme)?;
    Ok(())
}

/// 元値へ戻す。ユーザーが変えていたレールは触らない。
///
/// **呼んでよいのは [`run_action`] と [`clear_residual`] だけ**（どちらも
/// [`may_touch`] を通したうえで [`with_record`] のロックの下に居る）。
/// ここを直接呼ぶと #1373 の「他プロセスの記録を戻す」が復活する
fn restore(saved: &SavedLidState) -> Result<(), LidError> {
    let scheme = imp::guid_from_string(&saved.scheme)
        .ok_or_else(|| LidError::BadSchemeGuid(saved.scheme.clone()))?;
    for (rail, original) in saved.entries() {
        // 読めない（プランが消された等）なら諦めて記録だけ捨てる
        let Ok(current) = imp::read(&scheme, rail) else {
            continue;
        };
        if should_restore(current, original) {
            imp::write(&scheme, rail, original)?;
        }
    }
    imp::apply(&scheme)?;
    clear_saved();
    Ok(())
}

// --- OS をまたぐ口（#2857） ---
//
// 呼び手（`sleep_guard`）はこの節の口だけを呼ぶ。macOS の腕は #2857 より前に
// `sleep_guard.rs` の `#[cfg(target_os = "macos")]` の塊に並んでいたもの、macOS 以外の腕は
// 電源プランの腕（上の節）へそのまま渡す。判定（いつ倒す・いつ戻す）と記録（persist.log /
// stderr）は呼び手が持つ。
//
// #2904 で macOS の腕も倒した主の記録（`lid-guard.json` の `method: pmset`）を持つようになった。
// 記録の読み書き（写し・ロック・原子書き込み・壊れた記録の退避）と持ち主の判定（`claim_for`）は
// 電源プランの腕と同じ 1 実装を使い、倒す・戻すの判定だけ手段ごと（`pmset::decide`）に持つ。

/// macOS の手段（sudoers の限定登録 + `pmset disablesleep`。#218）
pub mod pmset;

#[cfg(target_os = "macos")]
use super::power::iokit;
#[cfg(target_os = "macos")]
use pmset::{is_sudoers_installed, set_disablesleep, PmsetAction};

/// いま `disablesleep` が倒れているか（IORegistry の `SleepDisabled`。検証プロセスで偽の置き場が
/// あればその値 = #2904）
#[cfg(target_os = "macos")]
fn pmset_disabled_now() -> bool {
    pmset::fake_sleep_disabled().unwrap_or_else(iokit::sleep_disabled)
}

/// `pmset` の記録に対するこのプロセスの立場（#2904）。記録が無ければ `None`。
/// 電源プランの記録（macOS では書かれない。別の機械から持ち込まれたもの）はこの機械の `pmset` とは
/// 無関係なので「記録なし」として読む（倒すときに上書きする）
#[cfg(target_os = "macos")]
fn pmset_claim(saved: Option<&SavedLidState>) -> Option<RecordClaim> {
    let saved = saved.filter(|s| s.method == LidMethod::Pmset)?;
    Some(claim_for(saved.owner.as_ref(), me(), probe_owner))
}

/// このプロセスが倒した記録（持ち主 = 自分）
#[cfg(target_os = "macos")]
fn pmset_record() -> SavedLidState {
    SavedLidState {
        method: LidMethod::Pmset,
        scheme: pmset::RECORD_SCHEME.to_string(),
        ac: None,
        dc: None,
        owner: Some(me().clone()),
    }
}

/// 自分のもの以外（持ち主の死んだ記録）へ手を出してよいかの門（#2904 の案 A）。
/// 材料は起動時の残留チェック（[`clear_startup_residual`] を呼ぶ `sleep_guard::check_disablesleep_residual`）
/// と同じ = 隔離されていない・他の tako が居ない
#[cfg(target_os = "macos")]
fn residual_gate_now() -> Result<(), &'static str> {
    pmset::residual_gate(
        tako_core::tmux_cleanup::is_isolated_value(std::env::var("TAKO_ISOLATED").ok().as_deref()),
        tako_core::ports::other_tako_running,
    )
}

/// `pmset` の上書きを要求（`enable`）に合わせる（#2904）。倒す・引き取る・戻す・記録を捨てるの 1 実装で、
/// 周期（[`apply_override`]）も終了（[`restore_on_exit`]）もここを通る。
///
/// 写しで判定し、何かすると決まったらロックの下で記録を読み直して同じ判定をやり直す（#1373 の作法）。
/// 実際にしたこと（何もしなければ [`PmsetAction::Nothing`]）を返す。記録を読めない・置き場が無いときは
/// `Err` で、**持ち主が分からないまま `pmset` を打たない**
#[cfg(target_os = "macos")]
fn settle_pmset(
    enable: bool,
    gate: fn() -> Result<(), &'static str>,
) -> Result<PmsetAction, String> {
    let cached = load_saved().map_err(LidError::into_ja)?;
    let claim = pmset_claim(cached.as_ref());
    // 戻したいのに記録が無い = 何もしない（設定 off の間の毎 tick を IORegistry まで行かせない）
    if !enable && claim.is_none() {
        return Ok(PmsetAction::Nothing);
    }
    if pmset::decide(enable, pmset_disabled_now(), claim, gate) == PmsetAction::Nothing {
        return Ok(PmsetAction::Nothing);
    }
    let path = state_path().ok_or_else(|| LidError::DataDirUnresolved.into_ja())?;
    with_record(&path, |fresh| {
        let action = pmset::decide(
            enable,
            pmset_disabled_now(),
            pmset_claim(fresh.as_ref()),
            gate,
        );
        Ok(run_pmset_action(action, &path).map(|()| action))
    })
    .map_err(LidError::into_ja)?
}

/// [`pmset::decide`] が返した操作を実行する（#2904）。**macOS の腕で `disablesleep 0` を打つのは
/// ここと起動時の残留チェック（[`clear_startup_residual`]）と A/B の旧い腕だけ**（番犬
/// `issue2904_lid_owner_watchdog`）。呼んでよいのは [`settle_pmset`] だけ（ロックの下で判定をやり直した後）
#[cfg(target_os = "macos")]
fn run_pmset_action(action: PmsetAction, path: &std::path::Path) -> Result<(), String> {
    match action {
        PmsetAction::Nothing => Ok(()),
        PmsetAction::Acquire => {
            // 記録を先に置く（倒したのに記録が無い = 戻す主の居ない上書き、を作らない。置いてから倒す
            // 前に落ちても、残るのは持ち主の死んだ記録といまの 0 = 次に誰かが捨てる）
            store_saved_at(path, &pmset_record()).map_err(LidError::into_ja)?;
            if let Err(e) = set_disablesleep(true) {
                clear_saved_at(Some(path));
                return Err(e);
            }
            Ok(())
        }
        PmsetAction::TakeOver => store_saved_at(path, &pmset_record()).map_err(LidError::into_ja),
        PmsetAction::Restore => {
            // 戻せなければ記録を残す（次の周期でやり直す。終了時なら次の起動の残留チェックが戻す）
            set_disablesleep(false)?;
            clear_saved_at(Some(path));
            Ok(())
        }
        PmsetAction::Forget => {
            clear_saved_at(Some(path));
            Ok(())
        }
    }
}

/// A/B の旧い腕（`TAKO_2904_LEGACY=1`）の周期: 記録を持たず、いまの値だけを見て倒す・戻す
#[cfg(target_os = "macos")]
fn legacy_2904_apply(enable: bool) -> Result<bool, String> {
    let current_disabled = pmset_disabled_now();
    if enable && !current_disabled {
        set_disablesleep(true).map(|()| true)
    } else if !enable && current_disabled {
        set_disablesleep(false).map(|()| true)
    } else {
        Ok(false)
    }
}

/// A/B の旧い腕（`TAKO_2904_LEGACY=1`）の終了: sudoers 登録済みで倒れていれば、誰のものかを見ずに戻す
#[cfg(target_os = "macos")]
fn legacy_2904_restore_on_exit() -> ExitRestore {
    if is_sudoers_installed() && pmset_disabled_now() && set_disablesleep(false).is_ok() {
        return ExitRestore::Restored;
    }
    ExitRestore::Nothing
}

/// いま効いている上書きの持ち主（#2904。`tako sleep-guard status` / MCP `tako_sleep_guard` の
/// `lid_override_holder`）。記録（`lid-guard.json`）に対する**このプロセス**の立場から決める
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LidHolder {
    /// このプロセスが倒した（設定 off・エージェントの一段落・終了で戻す）
    Mine,
    /// 同じ data dir の別の生きた tako が倒した（このプロセスは戻さない）
    OtherTako,
    /// 倒した tako はもう居ない（異常終了の残り。隔離でなく他の tako が居ないときに戻す）
    Stale,
    /// 記録が無い（tako 以外・別の data dir の tako・#2904 より前の版が倒した）。tako が戻すのは
    /// 起動時の残留チェックだけ
    NoRecord,
}

impl LidHolder {
    /// JSON の値（`lid_override_holder`）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mine => "self",
            Self::OtherTako => "other-tako",
            Self::Stale => "stale",
            Self::NoRecord => "no-record",
        }
    }

    /// [`Self::as_str`] の逆
    pub fn from_str_opt(s: &str) -> Option<Self> {
        [Self::Mine, Self::OtherTako, Self::Stale, Self::NoRecord]
            .into_iter()
            .find(|h| h.as_str() == s)
    }

    /// 記録に対する立場から（記録が無ければ `None`）
    pub fn of(claim: Option<RecordClaim>) -> Self {
        match claim {
            None => Self::NoRecord,
            Some(RecordClaim::Mine) => Self::Mine,
            Some(RecordClaim::Foreign { .. }) => Self::OtherTako,
            Some(RecordClaim::Abandoned | RecordClaim::Unowned) => Self::Stale,
        }
    }
}

/// いま効いている上書きの持ち主（#2904）。`active` は呼び手が読んだ [`override_active`] の値
/// （読み直さない）。効いていない・制御できない OS・記録を読めないときは `None`。
/// 記録は写しを読む（毎 tick のディスク読みを作らない）
pub fn override_holder(active: bool) -> Option<LidHolder> {
    if !active || !control_supported() {
        return None;
    }
    let saved = load_saved().ok()?;
    #[cfg(target_os = "macos")]
    let claim = pmset_claim(saved.as_ref());
    #[cfg(not(target_os = "macos"))]
    let claim = with_claim(saved.as_ref()).map(|(_, claim)| claim);
    Some(LidHolder::of(claim))
}

/// 蓋を閉じたまま走らせ続ける制御をこの OS が持つか（#524 / #697）。
/// 表の正本と経緯は `sleep_guard::lid_control_supported`
pub fn control_supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        crate::platform::lid::supported()
    }
}

/// 蓋の開閉（[`is_closed`]）をこの OS で観測できるか（#697。`sleep_guard::lid_state_detectable`）
pub fn state_detectable() -> bool {
    cfg!(target_os = "macos")
}

/// この OS の蓋閉じ継続が、管理者権限を伴う初回登録（macOS = sudoers）を要する仕組みか
/// （#697。`sleep_guard::lid_requires_privileged_setup`）。
///
/// 呼び手はこれを「sudoers の手段（[`pmset`]）を使う OS か」の能力として読む（呼び手に
/// `cfg` を戻さない = #2857）
pub fn requires_privileged_setup() -> bool {
    cfg!(target_os = "macos")
}

/// 蓋が閉じているか（macOS = IORegistry の `AppleClamshellState`。root 不要。#218）。
/// 観測できない OS は false（「開いている」と「分からない」の区別は [`state_detectable`]）
pub fn is_closed() -> bool {
    #[cfg(target_os = "macos")]
    {
        iokit::clamshell_closed()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// いま蓋閉じ継続の上書きが効いているか。
///
/// macOS = `pmset disablesleep` の現在値（IORegistry の `SleepDisabled`。root 不要。#218。
/// **誰が倒したかは問わない** = 持ち主は [`override_holder`]）、
/// Windows = 倒した記録が残っているか（[`is_active`]）
pub fn override_active() -> bool {
    #[cfg(target_os = "macos")]
    {
        pmset_disabled_now()
    }
    #[cfg(not(target_os = "macos"))]
    {
        is_active()
    }
}

/// 蓋閉じ継続の設定が off の間も、呼び手の周期（`sleep_guard::update`）で上書きを戻しに行くか（#2857）。
///
/// どちらの腕も倒した記録に所有者を持ち、設定が off なら**自分の上書きだけ**を戻す（Windows = #697 /
/// #1373、macOS = #2904）。記録の無い上書き・生きた他の tako の上書きは off でも戻さない。
/// A/B の旧い macOS の腕（`TAKO_2904_LEGACY=1`）だけは #218 からの挙動 = off の間は `pmset` に触らない
pub fn restores_while_mode_off() -> bool {
    #[cfg(target_os = "macos")]
    {
        !pmset::legacy()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// 蓋閉じ継続を倒す / 戻す。倒した・戻したら `Ok(true)`、同じ状態なら何もせず `Ok(false)`
/// （毎 tick 呼ばれる前提）。倒すべきかの判定は呼び手（`sleep_guard::lid_decision`）が持つ。
///
/// `include_battery` はバッテリー側も倒すか。電源プランは AC / DC を別々に持つので倒すレールを
/// 選ぶ（#1473）。macOS の `pmset -a` は電源を問わず効くので使わない。
///
/// どちらの腕も**自分が倒した上書きだけ**を戻す（Windows = #1373、macOS = #2904）。macOS で
/// `Ok(true)` になるのは `pmset` を実際に打ったとき（倒した・戻した）だけで、持ち主の死んだ上書きの
/// 引き取り・記録の掃除は `Ok(false)`
pub fn apply_override(enable: bool, include_battery: bool) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = include_battery;
        if pmset::legacy() {
            return legacy_2904_apply(enable);
        }
        settle_pmset(enable, residual_gate_now)
            .map(|done| matches!(done, PmsetAction::Acquire | PmsetAction::Restore))
    }
    #[cfg(not(target_os = "macos"))]
    {
        set_stay_awake(enable, include_battery)
    }
}

/// 蓋閉じ中のディスプレイ消灯（#311）。
///
/// macOS の `disablesleep=1` は蓋閉じスリープだけでなくディスプレイの消灯も止めるので、
/// 蓋閉じ + 倒している組み合わせでディスプレイだけを明示的に消す（蓋が閉じている間は入力が
/// 無いので 1 回送れば消灯が続く）。電源プランの lid action はディスプレイの消灯を妨げないので、
/// macOS 以外では何もしない
pub fn sync_display_sleep(lid_closed: bool, lid_sleep_disabled: bool) {
    #[cfg(target_os = "macos")]
    {
        // 偽の置き場（検証）の「倒れている」で本物の `pmset displaysleepnow` を打たない（#2904。
        // 蓋を閉じて外部ディスプレイで使っている利用者の画面を検証起動が消す）
        if lid_closed && lid_sleep_disabled && pmset::fake_sleep_disabled().is_none() {
            iokit::force_display_sleep();
        }
        if !lid_closed || !lid_sleep_disabled {
            iokit::reset_display_sleep_sent();
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (lid_closed, lid_sleep_disabled);
    }
}

/// 蓋閉じ中にディスプレイの消灯を送ったか（#311）
pub fn display_sleep_forced() -> bool {
    #[cfg(target_os = "macos")]
    {
        iokit::display_sleep_sent()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// 起動時の残留の後始末（[`clear_startup_residual`]）の結果（#449 / #697 / #2857）。
///
/// stderr の文と persist.log への記録は呼び手（`sleep_guard::check_disablesleep_residual`）が組む。
/// 文が手段ごとに違う（#2857 より前のまま）ので、手段の名前で分けて返す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupResidual {
    /// macOS: 戻さなかった（理由は [`pmset::should_clear_residual`] の文）
    PmsetSkipped(&'static str),
    /// macOS: `pmset disablesleep 0` で戻した
    PmsetCleared,
    /// macOS: 戻そうとして失敗した
    PmsetFailed(String),
    /// 電源プランの腕（Windows）: [`clear_residual`] の結果そのまま
    /// （`Ok(Some(説明))` = 戻した / `Ok(None)` = 触らない / `Err` = 失敗）
    PowerPlan(Result<Option<String>, String>),
}

/// 起動時の残留チェック: 前回のクラッシュ・異常終了で倒したまま残った上書きを元へ戻す（#449 / #697）。
///
/// 隔離（`is_isolated`）と他の tako の稼働（`other_instance_running`）の門は両方の腕が通る。
///
/// macOS の腕は記録の有無を問わず戻す（#449 のまま。記録を持たない #2904 より前の版が倒したまま
/// 落ちた残りも戻すため）。門を通った = 他の tako は居ないので、戻したら前回の記録も捨てる
pub fn clear_startup_residual(is_isolated: bool, other_instance_running: bool) -> StartupResidual {
    #[cfg(target_os = "macos")]
    {
        if let Err(reason) = pmset::should_clear_residual(
            is_isolated,
            other_instance_running,
            is_sudoers_installed(),
            pmset_disabled_now(),
        ) {
            return StartupResidual::PmsetSkipped(reason);
        }
        match set_disablesleep(false) {
            Err(e) => StartupResidual::PmsetFailed(e),
            Ok(()) => {
                clear_saved();
                StartupResidual::PmsetCleared
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        StartupResidual::PowerPlan(clear_residual(is_isolated, other_instance_running))
    }
}

/// 正常終了での戻しの結果（#2904。呼び手 `sleep_guard::cleanup_on_exit` が persist.log へ残す）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitRestore {
    /// 戻した（自分の上書き、または門を通った持ち主の死んだ残り）
    Restored,
    /// 上書きは効いているが戻さなかった（持ち主が自分ではない・門が閉じている）
    Kept(LidHolder),
    /// 戻すものが無かった（効いていない）
    Nothing,
}

/// tako の正常終了で蓋閉じ継続の上書きを戻す（#218 / #697 / #2904）。
///
/// **蓋の設定は OS が回収してくれない**（倒したまま終了すると、次に tako を起動するまで蓋を
/// 閉じてもスリープしない）。起動時の残留復元（[`clear_startup_residual`]）は最後の砦であって、
/// 正常終了ではここで戻す。
///
/// **戻すのは自分が倒した上書きだけ**（#2904）。`disablesleep` は機械全体の設定なので、#2904 より前の
/// macOS の腕は検証起動の tako の終了でも本番の GUI が倒した上書きを 0 へ戻していた。持ち主の死んだ
/// 記録は起動時と同じ門（[`residual_gate_now`]）を通ったときだけ戻す。macOS は終了が 1 回きりなので
/// 記録の写しを信じずディスクから読み直す。#2904 より前は macOS の腕の失敗を黙って捨てていたが、
/// いまは `Err` を返す（呼び手が stderr と persist.log へ残す）
pub fn restore_on_exit() -> Result<ExitRestore, String> {
    #[cfg(target_os = "macos")]
    {
        if pmset::legacy() {
            return Ok(legacy_2904_restore_on_exit());
        }
        invalidate_cache();
        if settle_pmset(false, residual_gate_now)? == PmsetAction::Restore {
            return Ok(ExitRestore::Restored);
        }
        Ok(match override_holder(pmset_disabled_now()) {
            Some(holder) => ExitRestore::Kept(holder),
            None => ExitRestore::Nothing,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        if set_stay_awake(false, false)? {
            return Ok(ExitRestore::Restored);
        }
        Ok(match override_holder(is_active()) {
            Some(holder) => ExitRestore::Kept(holder),
            None => ExitRestore::Nothing,
        })
    }
}

#[cfg(windows)]
mod imp {
    use super::{LidError, Rail};
    use std::ffi::c_void;

    pub(super) const SUPPORTED: bool = true;

    /// `GUID`（guiddef.h）
    #[repr(C)]
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(super) struct Guid {
        pub data1: u32,
        pub data2: u16,
        pub data3: u16,
        pub data4: [u8; 8],
    }

    /// `GUID_SYSTEM_BUTTON_SUBGROUP`（powrprof の SUB_BUTTONS）
    const SUB_BUTTONS: Guid = Guid {
        data1: 0x4f97_1e89,
        data2: 0xeebd,
        data3: 0x4455,
        data4: [0xa8, 0xde, 0x9e, 0x59, 0x04, 0x0e, 0x73, 0x47],
    };

    /// `GUID_LIDCLOSE_ACTION`。
    ///
    /// この設定は定義側の `Attributes = 1`（UI から hidden）なので
    /// `powercfg /q` の一覧には出てこないが、**GUID を明示すれば読み書きできる**
    /// （実測。#697 の Issue 本文に採取ログ）
    const LIDCLOSE_ACTION: Guid = Guid {
        data1: 0x5ca8_3367,
        data2: 0x6e45,
        data3: 0x459f,
        data4: [0xa2, 0x7b, 0x47, 0x6b, 0x1d, 0x01, 0xc9, 0x36],
    };

    const ERROR_SUCCESS: u32 = 0;

    #[link(name = "powrprof")]
    extern "system" {
        fn PowerGetActiveScheme(user_root: *mut c_void, scheme: *mut *mut Guid) -> u32;
        fn PowerSetActiveScheme(user_root: *mut c_void, scheme: *const Guid) -> u32;
        fn PowerReadACValueIndex(
            root: *mut c_void,
            scheme: *const Guid,
            subgroup: *const Guid,
            setting: *const Guid,
            value: *mut u32,
        ) -> u32;
        fn PowerReadDCValueIndex(
            root: *mut c_void,
            scheme: *const Guid,
            subgroup: *const Guid,
            setting: *const Guid,
            value: *mut u32,
        ) -> u32;
        fn PowerWriteACValueIndex(
            root: *mut c_void,
            scheme: *const Guid,
            subgroup: *const Guid,
            setting: *const Guid,
            value: u32,
        ) -> u32;
        fn PowerWriteDCValueIndex(
            root: *mut c_void,
            scheme: *const Guid,
            subgroup: *const Guid,
            setting: *const Guid,
            value: u32,
        ) -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    /// 現在アクティブな電源プランの GUID
    pub(super) fn active_scheme() -> Result<Guid, LidError> {
        let mut ptr: *mut Guid = std::ptr::null_mut();
        // SAFETY: 出力先はスタック上のポインタ。成功時のみ中身を読む
        let rc = unsafe { PowerGetActiveScheme(std::ptr::null_mut(), &mut ptr) };
        if rc != ERROR_SUCCESS || ptr.is_null() {
            return Err(LidError::ActiveScheme(rc));
        }
        // SAFETY: rc が成功 かつ 非 NULL。API 仕様どおり LocalFree で解放する
        let guid = unsafe { *ptr };
        // SAFETY: PowerGetActiveScheme が確保したバッファ。解放は LocalFree が正
        unsafe { LocalFree(ptr as *mut c_void) };
        Ok(guid)
    }

    pub(super) fn read(scheme: &Guid, rail: Rail) -> Result<u32, LidError> {
        let mut value: u32 = 0;
        // SAFETY: 入力はすべて生存する参照、出力先はスタック上の u32
        let rc = unsafe {
            match rail {
                Rail::Ac => PowerReadACValueIndex(
                    std::ptr::null_mut(),
                    scheme,
                    &SUB_BUTTONS,
                    &LIDCLOSE_ACTION,
                    &mut value,
                ),
                Rail::Dc => PowerReadDCValueIndex(
                    std::ptr::null_mut(),
                    scheme,
                    &SUB_BUTTONS,
                    &LIDCLOSE_ACTION,
                    &mut value,
                ),
            }
        };
        if rc != ERROR_SUCCESS {
            return Err(LidError::ReadSetting { rail, rc });
        }
        Ok(value)
    }

    pub(super) fn write(scheme: &Guid, rail: Rail, value: u32) -> Result<(), LidError> {
        // SAFETY: 入力はすべて生存する参照
        let rc = unsafe {
            match rail {
                Rail::Ac => PowerWriteACValueIndex(
                    std::ptr::null_mut(),
                    scheme,
                    &SUB_BUTTONS,
                    &LIDCLOSE_ACTION,
                    value,
                ),
                Rail::Dc => PowerWriteDCValueIndex(
                    std::ptr::null_mut(),
                    scheme,
                    &SUB_BUTTONS,
                    &LIDCLOSE_ACTION,
                    value,
                ),
            }
        };
        if rc != ERROR_SUCCESS {
            // 5 = ERROR_ACCESS_DENIED。グループポリシーで固定されている環境が該当
            return Err(LidError::WriteSetting { rail, rc });
        }
        Ok(())
    }

    /// 書いた値を有効化する。**これを呼ばないと反映されない**
    pub(super) fn apply(scheme: &Guid) -> Result<(), LidError> {
        // SAFETY: scheme は生存する参照
        let rc = unsafe { PowerSetActiveScheme(std::ptr::null_mut(), scheme) };
        if rc != ERROR_SUCCESS {
            return Err(LidError::ApplyScheme(rc));
        }
        Ok(())
    }

    pub(super) fn guid_to_string(g: &Guid) -> String {
        super::guid_fmt(g.data1, g.data2, g.data3, &g.data4)
    }

    pub(super) fn guid_from_string(s: &str) -> Option<Guid> {
        let (data1, data2, data3, data4) = super::guid_parse(s)?;
        Some(Guid {
            data1,
            data2,
            data3,
            data4,
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{LidError, Rail};

    /// macOS は OS をまたぐ口の macOS の腕（clamshell + [`super::pmset`]）が担当するので
    /// 電源プランの腕は使わない。Linux 等は蓋の動作を触る共通の仕組みが無い
    pub(super) const SUPPORTED: bool = false;

    /// 非 Windows では GUID を扱わないので、型だけ合わせた空実装。
    ///
    /// `()` の別名にすると呼び出し側の `let scheme = active_scheme()?` が
    /// 「unit を束縛している」と clippy に叱られる（`let_unit_value`）。
    /// **境界の都合を呼び出し側の `allow` へ漏らさない**ために、
    /// ここでサイズゼロの専用型を持つ
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct Guid;

    pub(super) fn active_scheme() -> Result<Guid, LidError> {
        Err(LidError::Unsupported)
    }
    pub(super) fn read(_scheme: &Guid, _rail: Rail) -> Result<u32, LidError> {
        Err(LidError::Unsupported)
    }
    pub(super) fn write(_scheme: &Guid, _rail: Rail, _value: u32) -> Result<(), LidError> {
        Err(LidError::Unsupported)
    }
    pub(super) fn apply(_scheme: &Guid) -> Result<(), LidError> {
        Err(LidError::Unsupported)
    }
    pub(super) fn guid_to_string(_g: &Guid) -> String {
        String::new()
    }
    pub(super) fn guid_from_string(_s: &str) -> Option<Guid> {
        None
    }
}

// --- GUID の文字列化 / 解釈（`cfg` の外。**Windows 実機が無くてもテストできる**） ---

/// `PowerGetActiveScheme` が返す GUID を `powercfg` と同じ表記へ。
///
/// 呼ぶのは Windows の `imp` とテストだけなので、macOS の通常ビルド
/// （`--all-targets` を付けない `cargo build`）では未使用になる
#[cfg_attr(not(windows), allow(dead_code))]
fn guid_fmt(data1: u32, data2: u16, data3: u16, data4: &[u8; 8]) -> String {
    format!(
        "{data1:08x}-{data2:04x}-{data3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        data4[0], data4[1], data4[2], data4[3], data4[4], data4[5], data4[6], data4[7]
    )
}

/// `guid_fmt` の逆。壊れた記録は `None`（呼び出し側は残留復元を諦める）
#[cfg_attr(not(windows), allow(dead_code))]
fn guid_parse(s: &str) -> Option<(u32, u16, u16, [u8; 8])> {
    let parts: Vec<&str> = s.trim().split('-').collect();
    if parts.len() != 5 || parts[0].len() != 8 || parts[1].len() != 4 || parts[2].len() != 4 {
        return None;
    }
    if parts[3].len() != 4 || parts[4].len() != 12 {
        return None;
    }
    let data1 = u32::from_str_radix(parts[0], 16).ok()?;
    let data2 = u16::from_str_radix(parts[1], 16).ok()?;
    let data3 = u16::from_str_radix(parts[2], 16).ok()?;
    let tail = format!("{}{}", parts[3], parts[4]);
    let mut data4 = [0u8; 8];
    for (i, slot) in data4.iter_mut().enumerate() {
        *slot = u8::from_str_radix(tail.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some((data1, data2, data3, data4))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- 記録の所有権（#1373） ---

    /// 所有者を作るヘルパー。実在するプロセスである必要はない（判定は `probe` が担う）
    fn owner(pid: u32, started: Option<u64>) -> RecordOwner {
        RecordOwner {
            pid,
            started_unix: started,
        }
    }

    /// 記録を 1 件でっち上げる。所有者以外は判定に効かない
    fn record_owned_by(o: Option<RecordOwner>) -> SavedLidState {
        SavedLidState {
            method: LidMethod::PowerPlan,
            scheme: "381b4222-f694-41f0-9685-ff5bb260df2e".to_string(),
            ac: Some(1),
            dc: None,
            owner: o,
        }
    }

    /// 疑似プロセスの状態を返す `probe`。実プロセスを起こさずに 4 通りを固定する
    fn probe_of(state: Owner) -> impl Fn(u32) -> Owner {
        move |_| state
    }

    fn alive_at(secs: u64) -> Owner {
        Owner::Alive {
            started: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
        }
    }

    #[test]
    fn 所有者の同一判定は起動時刻まで見る() {
        let me = owner(100, Some(1_700_000_000));
        assert!(me.is_same(&owner(100, Some(1_700_000_000))));
        assert!(
            !me.is_same(&owner(100, Some(1_700_000_500))),
            "pid が同じでも起動時刻が違えば別プロセス（pid の再利用）"
        );
        assert!(!me.is_same(&owner(101, Some(1_700_000_000))));
        assert!(
            owner(100, None).is_same(&owner(100, Some(1))),
            "起動時刻が取れない OS では pid だけで一致とみなす（自分の記録を戻せなくなる方が害が大きい）"
        );
    }

    /// #1373 の受け入れ条件 1（その 1）: **他プロセスが保持している記録は解除で戻さない**。
    ///
    /// 修正前は `set_stay_awake(false, …)` が記録の出所を見ずに `restore()` していたので、
    /// busy な A が倒した上書きを idle な B が戻していた（#449 と同型）
    #[test]
    fn 他プロセスが保持している記録は解除で戻さない() {
        let me = owner(100, Some(1_700_000_000));
        let rec = record_owned_by(Some(owner(200, Some(1_700_000_100))));

        let claim = claim_for(rec.owner.as_ref(), &me, probe_of(alive_at(1_700_000_100)));
        assert_eq!(claim, RecordClaim::Foreign { pid: 200 });
        assert!(!may_touch(claim));
        assert_eq!(
            decide(false, rails_for(false), Some((&rec, claim))),
            LidAction::Nothing,
            "解除では他人の記録に触らない"
        );
        assert_eq!(
            decide(true, rails_for(false), Some((&rec, claim))),
            LidAction::Nothing,
            "倒す側も他人の記録を書き換えない（相手が解除した次の tick で取り直す）"
        );
    }

    /// 受け入れ条件 1（その 2）: **自分が書いた記録は戻す**
    #[test]
    fn 自分の記録は解除で戻す() {
        let me = owner(100, Some(1_700_000_000));
        let rec = record_owned_by(Some(me.clone()));

        let claim = claim_for(rec.owner.as_ref(), &me, probe_of(Owner::Dead));
        assert_eq!(claim, RecordClaim::Mine, "所有者の生死を引くまでもない");
        assert_eq!(
            decide(false, rails_for(false), Some((&rec, claim))),
            LidAction::Restore
        );
        assert_eq!(
            decide(true, rails_for(false), Some((&rec, claim))),
            LidAction::Nothing,
            "過不足なく倒しているので再要求は no-op"
        );
        assert_eq!(
            decide(true, rails_for(true), Some((&rec, claim))),
            LidAction::RestoreThenAcquire,
            "電源条件が ac-only → always へ変わったら倒し直す"
        );
    }

    /// 受け入れ条件 1（その 3）: **所有者が死んでいる記録は戻す**（残留の回収）
    #[test]
    fn 死んだ所有者の記録は戻す() {
        let me = owner(100, Some(1_700_000_000));
        let rec = record_owned_by(Some(owner(200, Some(1_700_000_100))));

        let claim = claim_for(rec.owner.as_ref(), &me, probe_of(Owner::Dead));
        assert_eq!(claim, RecordClaim::Abandoned);
        assert!(may_touch(claim));
        assert_eq!(
            decide(false, rails_for(false), Some((&rec, claim))),
            LidAction::Restore
        );
    }

    /// pid が生きていても**起動時刻が食い違えば別人**（pid の再利用）
    #[test]
    fn pidの再利用は所有者が死んだものとして扱う() {
        let me = owner(100, Some(1_700_000_000));
        let rec = record_owned_by(Some(owner(200, Some(1_700_000_100))));

        // いま pid 200 で走っているのは、記録より後に始まった別プロセス
        let claim = claim_for(rec.owner.as_ref(), &me, probe_of(alive_at(1_700_009_999)));
        assert_eq!(claim, RecordClaim::Abandoned);
    }

    /// 生死を判定できないときは**触らない側**へ倒す
    #[test]
    fn 生死が分からない所有者の記録には触らない() {
        let me = owner(100, Some(1_700_000_000));
        let rec = record_owned_by(Some(owner(200, Some(1_700_000_100))));

        for state in [Owner::Unknown, Owner::Alive { started: None }] {
            let claim = claim_for(rec.owner.as_ref(), &me, probe_of(state));
            assert_eq!(
                claim,
                RecordClaim::Foreign { pid: 200 },
                "{state:?} は「死んだ」と言い切れない"
            );
        }
    }

    /// #1373 より前に書かれた記録（所有者なし）は従来どおり戻す。
    /// 引き取りは**必ず「戻してから倒し直す」**（倒れたままの 0 を元値として書かない）
    #[test]
    fn 所有者の居ない旧形式の記録は引き取る() {
        let me = owner(100, Some(1_700_000_000));
        let rec = record_owned_by(None);

        let claim = claim_for(rec.owner.as_ref(), &me, probe_of(Owner::Dead));
        assert_eq!(claim, RecordClaim::Unowned);
        assert_eq!(
            decide(false, rails_for(false), Some((&rec, claim))),
            LidAction::Restore
        );
        assert_eq!(
            decide(true, rails_for(false), Some((&rec, claim))),
            LidAction::RestoreThenAcquire,
            "所有者を引き取るときも現在値（倒れた 0）を元値として記録し直さない"
        );
    }

    /// 記録が無いときの判定
    #[test]
    fn 記録が無ければ倒すときだけ書く() {
        assert_eq!(decide(true, rails_for(false), None), LidAction::Acquire);
        assert_eq!(decide(false, rails_for(false), None), LidAction::Nothing);
    }

    /// 旧形式（`owner` を持たない `lid-guard.json`）が**そのまま読める**こと。
    /// 読めるので移行手順は要らない（#916 の「serde の default で足りる」側）
    #[test]
    fn 旧形式の記録がそのまま読める() {
        let old = r#"{"scheme":"381b4222-f694-41f0-9685-ff5bb260df2e","ac":1,"dc":null}"#;
        let state: SavedLidState = serde_json::from_str(old).expect("旧形式が読める");
        assert_eq!(state.ac, Some(1));
        assert_eq!(state.owner, None, "所有者を持たない記録として読める");
    }

    /// 実プロセスに対する `probe_owner` の答え（純粋判定と実機の橋渡し）。
    ///
    /// 自分の pid は必ず生きていて起動時刻が取れる。看取った pid は、看取った子の押さえを
    /// 持つ間（= 別のプロセスに再利用されない間）は `Dead`（#2284）
    #[test]
    fn 実プロセスの生死を引ける() {
        let self_pid = std::process::id();
        let started = match probe_owner(self_pid) {
            Owner::Alive { started } => started,
            other => panic!("自分の pid が生きていないと判定された: {other:?}"),
        };
        assert_eq!(
            started.and_then(unix_secs),
            tako_core::platform::procinfo::start_time_unix(self_pid),
            "起動時刻は procinfo の値をそのまま運ぶ"
        );
        // 自分が書いた記録は、実 probe でも `Mine`
        let rec = record_owned_by(Some(RecordOwner::current()));
        assert_eq!(
            claim_for(rec.owner.as_ref(), me(), probe_owner),
            RecordClaim::Mine
        );

        // 看取った pid = 確実に死んでいる（大きい適当な数は再利用中の生者に当たりうる）。
        // **読み終わるまで押さえを持つ**: 捨てると Windows はその pid を別のプロセスへ
        // 再利用しうる（#2284）
        let scratch = tako_core::test_residue::ScratchDir::new("tako-lid-reap");
        let dead = reaped_child(scratch.path());
        let dead_pid = dead.pid();
        assert_eq!(probe_owner(dead_pid), Owner::Dead);
        let rec = record_owned_by(Some(owner(dead_pid, Some(1_700_000_000))));
        let claim = claim_for(rec.owner.as_ref(), me(), probe_owner);
        assert_eq!(claim, RecordClaim::Abandoned);
        assert_eq!(
            decide(false, rails_for(false), Some((&rec, claim))),
            LidAction::Restore,
            "死んだ所有者の残留は実 probe でも回収する"
        );
        drop(dead);
    }

    /// 「確実に死んでいる pid」を作る（起こして看取る）。
    /// 子はこのテストバイナリ自身で、**どのテストにも一致しないフィルタ**を渡すので
    /// 0 件走って即終了する。一時ファイルの置き場は使い捨てへ向ける（#1296 / #1312）。
    ///
    /// pid が死んだままなのは**返した押さえを持つ間だけ**（#2284）。Windows は看取った子の
    /// ハンドルが閉じた直後に pid を別のプロセスへ再利用しうるので、押さえを捨ててから読むと
    /// pid を取った別プロセスの生存（`Alive`）を読む
    fn reaped_child(scratch: &std::path::Path) -> tako_core::platform::process::ReapedChild {
        let exe = std::env::current_exe().expect("テストバイナリのパス");
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["--exact", "__tako_lid_no_such_test__", "--test-threads=1"])
            .env("TMPDIR", scratch)
            .env("TMP", scratch)
            .env("TEMP", scratch)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Windows でもコンソール窓を出さない（#628 / #586 の門を通す）
        let child = tako_core::platform::process::ReapedChild::run(
            tako_core::platform::process::no_console_window(&mut cmd),
        )
        .expect("看取り用の子を起こして看取れる");
        let pid = child.pid();
        // 看取った直後の 1 回の観測で決めない（#2275）
        assert!(
            tako_core::platform::process::wait_gone(pid, std::time::Duration::from_secs(10)),
            "看取った pid {pid} が居なくならない"
        );
        child
    }

    /// 注入した子で回す [`実プロセスの生死を引ける`] の `--exact` 名
    const REAL_PROBE_TEST: &str = "platform::lid::tests::実プロセスの生死を引ける";

    /// **#2284 の名指し**: 終わったのにプロセスオブジェクトが残る pid（Windows で誰かが
    /// ハンドルを持つ間。起動時刻は読める）を `Owner::Alive` と読まない。
    ///
    /// 修正前の `probe_owner` は起動時刻を先に読み、取れたら `pid_alive`（終了コードを
    /// 先に問う = #2275）を通らずに `Alive` を返した。Windows CI では看取った pid に
    /// 誰かのハンドルが残った回だけ落ちる（間欠）。その状態を注入
    /// （`TAKO_2284_INJECT_LINGERING=1`）で毎回作った子で [`実プロセスの生死を引ける`] を回す
    /// = macOS の CI でもこの経路を毎回試す
    #[test]
    fn issue2284_終わったのに起動時刻が読めるpidをaliveと読まない() {
        let (ok, stdout) = run_real_probe_injected(
            tako_core::platform::procinfo::ENV_INJECT_2284,
            "tako-lid-2284",
        );
        assert!(
            ok,
            "終わったのに起動時刻が読める pid を Alive と読んだ（#2284）:\n{stdout}"
        );
    }

    /// **#2284 の名指し（10/24 の再発）**: 看取った pid を、別のプロセスに再利用されうる状態で
    /// 読まない（[`実プロセスの生死を引ける`] の「看取った pid は `Dead`」の前提を守る）。
    ///
    /// Windows は看取った子のハンドルが閉じた直後に pid を別のプロセスへ再利用しうる。
    /// 看取った子（`Child`）を drop してから `probe_owner` を読むと、その間に pid を取った
    /// 別プロセスの生存（`Alive { started: Some(看取りと同じ秒) }`）を読んで落ちる
    /// （Windows CI の間欠）。看取りを確かめた直後の再利用を注入
    /// （`TAKO_2284_INJECT_REUSE=1`）で毎回作った子で [`実プロセスの生死を引ける`] を回す
    #[test]
    fn issue2284_看取ったpidを再利用されうる状態で読まない() {
        let (ok, stdout) = run_real_probe_injected(
            tako_core::platform::procinfo::ENV_INJECT_2284_REUSE,
            "tako-lid-2284r",
        );
        assert!(
            ok,
            "看取った pid を、別のプロセスに再利用されうる状態で読んだ（#2284）:\n{stdout}"
        );
    }

    /// 注入（`env` = `1`）を付けた子で [`実プロセスの生死を引ける`] を回し、
    /// （通ったか, 子の stdout）を返す。子でそのテストが走っていなければここで落とす
    fn run_real_probe_injected(env: &str, scratch_name: &str) -> (bool, String) {
        let scratch = tako_core::test_residue::ScratchDir::new(scratch_name);
        let exe = std::env::current_exe().expect("テストバイナリのパス");
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["--exact", REAL_PROBE_TEST, "--test-threads=1"])
            .env(env, "1")
            .env("TMPDIR", scratch.path())
            .env("TMP", scratch.path())
            .env("TEMP", scratch.path());
        let out = tako_core::platform::process::no_console_window(&mut cmd)
            .output()
            .expect("注入した子を起こせる");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            stdout.contains("1 passed") || stdout.contains("1 failed"),
            "子で {REAL_PROBE_TEST} が走っていない（名前の取り違え）:\n{stdout}"
        );
        (out.status.success(), stdout)
    }

    // --- 記録の入出力（#1373 の症状 2） ---

    /// 壊れた記録を**「記録なし」へ丸めない**こと。
    ///
    /// 丸めると #169 と同じ三段連鎖で、倒れたままの現在値（0）を元値として記録し直し、
    /// ユーザーの蓋設定が永久に失われる。退避先は `<name>.unreadable.bak` で
    /// **元のファイルは触らない**（#916 の作法）
    #[test]
    fn 壊れた記録は記録なしへ丸めず退避してエラーになる() {
        let scratch = tako_core::test_residue::ScratchDir::new("tako-lid-broken");
        for (tag, body) in [
            ("empty", ""),
            ("truncated", r#"{"scheme":"381b4222-f694-41f0-9685-ff5b"#),
            ("garbage", "not json at all"),
        ] {
            let path = scratch.path().join(format!("lid-guard-{tag}.json"));
            std::fs::write(&path, body).expect("壊れた記録を置ける");
            let err = read_from(&path).expect_err("記録なしへ丸めない");
            let quarantine = tako_core::migration::quarantine_path(&path);
            // 日本語は #2365 より前の組み立て（`json_error_reason` + 退避先）とバイト一致
            let serde_err = serde_json::from_str::<SavedLidState>(body).expect_err("壊れている");
            let old_ja = format!(
                "蓋閉じ継続の記録を解釈できません（{}）: {}。内容は {} へ退避しました",
                path.display(),
                tako_core::load_issue::json_error_reason(&serde_err),
                quarantine.display()
            );
            let ja = err.text_in(tako_core::i18n::Lang::Ja);
            assert_eq!(ja, old_ja, "理由が #2365 より前と違う（{tag}）");
            // 英語表示は理由も退避先の言い方も英語（パスは表記のまま）
            let en = err.text_in(tako_core::i18n::Lang::En);
            assert!(
                en.starts_with("cannot parse the lid-close record (")
                    && en.contains("; its contents were saved to ")
                    && !en.contains("JSON の"),
                "英語表示に日本語が混ざる（{tag}）: {en}"
            );
            assert_eq!(
                std::fs::read_to_string(&quarantine).ok().as_deref(),
                Some(body),
                "退避先へ丸ごと写っている（{tag}）"
            );
            assert_eq!(
                std::fs::read_to_string(&path).ok().as_deref(),
                Some(body),
                "元のファイルは触らない（{tag}）"
            );
            // 2 度目も同じ（退避は冪等・上書きしない）
            assert!(read_from(&path).is_err(), "読み直しても丸めない（{tag}）");
        }
    }

    /// 記録の書き込みが**原子的**であること。
    /// tmp + rename を通るので、並行プロセスの読み手には旧内容か新内容しか見えない
    #[test]
    fn 記録の書き込みは原子書き込みを通る() {
        let _serial = crate::platform::testing::machine_state_lock();
        let scratch = tako_core::test_residue::ScratchDir::new("tako-lid-atomic");
        let path = scratch.path().join("lid-guard.json");
        invalidate_cache();

        let state = record_owned_by(Some(RecordOwner::current()));
        store_saved_at(&path, &state).expect("保存できる");

        let leftovers: Vec<String> = std::fs::read_dir(scratch.path())
            .expect("読める")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "一時ファイルが残っている: {leftovers:?}"
        );
        assert_eq!(
            serde_json::from_str::<SavedLidState>(&std::fs::read_to_string(&path).expect("読める"))
                .expect("完全な JSON が置かれている"),
            state
        );
        invalidate_cache();
    }

    /// 書くと決まったときだけロックを取り、**その下で読み直す**こと（#1373）。
    ///
    /// 写しは他プロセスの書き込みを知らないので、ロックの下では必ずディスクが正になる
    #[test]
    fn 書く経路はロックの下でディスクを読み直す() {
        let _serial = crate::platform::testing::machine_state_lock();
        let scratch = tako_core::test_residue::ScratchDir::new("tako-lid-lock");
        let path = scratch.path().join("lid-guard.json");
        let lock = path.with_file_name("lid-guard.json.lock");
        invalidate_cache();

        // 読むだけの経路ではロックファイルを作らない
        assert_eq!(load_saved_from(Some(&path)), Ok(None), "記録なしから始まる");
        assert!(!lock.exists(), "読むだけでロックファイルを増やさない");

        // 他プロセスが置いた記録（写しは「記録なし」のまま）
        let outside = record_owned_by(Some(owner(4_242, Some(1_700_000_000))));
        std::fs::write(&path, serde_json::to_string(&outside).expect("書ける")).expect("置ける");
        assert_eq!(
            load_saved_from(Some(&path)),
            Ok(None),
            "写しはまだ古い（毎 tick 読まない設計なので当然）"
        );

        let seen = with_record(&path, Ok).expect("ロックを取れる");
        assert_eq!(seen.as_ref(), Some(&outside), "ロックの下で読み直している");
        assert!(lock.exists(), "書く経路はロックを取る");
        assert_eq!(
            load_saved_from(Some(&path)),
            Ok(Some(outside)),
            "写しも読み直した内容へ更新される"
        );
        invalidate_cache();
    }

    #[test]
    fn 電源条件でレールが決まる() {
        assert_eq!(rails_for(false), &[Rail::Ac], "ac-only は AC だけ倒す");
        assert_eq!(rails_for(true), &[Rail::Ac, Rail::Dc], "always は両方倒す");
    }

    #[test]
    fn 我々が倒した値なら元へ戻す() {
        // 現在値が 0（我々が書いた「何もしない」）で、元が 1（スリープ）
        assert!(should_restore(0, 1));
        assert!(should_restore(0, 2), "元が休止状態でも戻す");
    }

    #[test]
    fn ユーザーが変えていたら戻さない() {
        // 倒したあとユーザーが「休止状態」へ変えた → 触らない
        assert!(!should_restore(2, 1));
        assert!(!should_restore(1, 1));
    }

    #[test]
    fn もともと何もしない設定なら戻す必要がない() {
        // 元値が 0 の人は倒しても値が変わらない。戻す操作自体が no-op
        assert!(!should_restore(0, 0));
    }

    #[test]
    fn 残留解除の判定() {
        assert!(should_clear_residual(false, false, true).is_ok());
        assert!(
            should_clear_residual(true, false, true).is_err(),
            "隔離モードは触らない"
        );
        assert!(
            should_clear_residual(false, true, true).is_err(),
            "他インスタンスが倒している最中かもしれない"
        );
        assert!(
            should_clear_residual(false, false, false).is_err(),
            "記録が無ければ残留なし"
        );
    }

    #[test]
    fn guidの文字列化と解釈が往復する() {
        // 実機のバランスプラン
        let s = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let (d1, d2, d3, d4) = guid_parse(s).expect("解釈できる");
        assert_eq!(d1, 0x381b_4222);
        assert_eq!(d2, 0xf694);
        assert_eq!(d3, 0x41f0);
        assert_eq!(guid_fmt(d1, d2, d3, &d4), s, "往復して同じ文字列に戻る");
    }

    #[test]
    fn 壊れたguidは解釈しない() {
        assert!(guid_parse("").is_none());
        assert!(guid_parse("381b4222").is_none());
        assert!(guid_parse("381b4222-f694-41f0-9685").is_none());
        assert!(
            guid_parse("zzzzzzzz-f694-41f0-9685-ff5bb260df2e").is_none(),
            "16 進でない"
        );
        assert!(
            guid_parse("381b422-f694-41f0-9685-ff5bb260df2e").is_none(),
            "桁数が足りない"
        );
    }

    #[test]
    fn 記録の記法が往復する() {
        let state = SavedLidState {
            method: LidMethod::PowerPlan,
            scheme: "381b4222-f694-41f0-9685-ff5bb260df2e".to_string(),
            ac: Some(1),
            dc: None,
            owner: Some(RecordOwner {
                pid: 4_242,
                started_unix: Some(1_700_000_000),
            }),
        };
        let text = serde_json::to_string(&state).expect("書ける");
        let back: SavedLidState = serde_json::from_str(&text).expect("読める");
        assert_eq!(back, state);
        assert_eq!(back.entries(), vec![(Rail::Ac, 1)]);
        assert!(back.covers(Rail::Ac));
        assert!(!back.covers(Rail::Dc), "DC は倒していない");
    }

    /// 電源条件の切り替えで倒しっぱなしを作らないこと（#697）。
    /// 「足りているか」で判定すると always → ac-only のときに DC が残る
    #[test]
    fn レールの一致は過不足なく見る() {
        let ac_only = SavedLidState {
            method: LidMethod::PowerPlan,
            scheme: "x".to_string(),
            ac: Some(1),
            dc: None,
            owner: None,
        };
        let both = SavedLidState {
            method: LidMethod::PowerPlan,
            scheme: "x".to_string(),
            ac: Some(1),
            dc: Some(1),
            owner: None,
        };

        assert!(
            ac_only.covers_exactly(rails_for(false)),
            "ac-only 同士は一致"
        );
        assert!(both.covers_exactly(rails_for(true)), "always 同士は一致");

        assert!(
            !ac_only.covers_exactly(rails_for(true)),
            "ac-only → always は倒し足りない"
        );
        assert!(
            !both.covers_exactly(rails_for(false)),
            "always → ac-only は DC が余る（ここを見落とすと倒しっぱなしになる）"
        );
    }

    #[test]
    fn 両レールを倒した記録() {
        let state = SavedLidState {
            method: LidMethod::PowerPlan,
            scheme: "x".to_string(),
            ac: Some(1),
            dc: Some(2),
            owner: None,
        };
        assert_eq!(state.entries(), vec![(Rail::Ac, 1), (Rail::Dc, 2)]);
        assert!(state.covers(Rail::Ac) && state.covers(Rail::Dc));
    }

    /// 非対応 OS では倒す操作が無害に素通りする（macOS CI で常に通る）
    #[test]
    fn 非対応osでは素通りする() {
        let _serial = crate::platform::testing::machine_state_lock();
        if !supported() {
            assert!(!is_active());
            assert_eq!(set_stay_awake(true, false), Ok(false));
            assert_eq!(clear_residual(false, false), Ok(None));
        }
    }

    /// 実機で電源プランの蓋設定を読み書きできることの確認（#697 の一次証拠）。
    ///
    /// **復元を assert より先に行う**こと。途中で落ちると
    /// 「蓋を閉じてもスリープしない」設定がユーザーの機械に残ってしまう
    #[cfg(windows)]
    #[test]
    fn 実機で蓋の設定を倒して元へ戻せる() {
        let _serial = crate::platform::testing::machine_state_lock();
        let scheme = imp::active_scheme().expect("アクティブな電源プランが取れる");
        let original = imp::read(&scheme, Rail::Ac).expect("AC 側の蓋設定が読める");

        imp::write(&scheme, Rail::Ac, LID_ACTION_DO_NOTHING).expect("倒せる");
        imp::apply(&scheme).expect("適用できる");
        let after = imp::read(&scheme, Rail::Ac).expect("読み戻せる");

        // 検査より先に必ず元へ戻す
        let restored = imp::write(&scheme, Rail::Ac, original).and_then(|()| imp::apply(&scheme));

        assert_eq!(
            after, LID_ACTION_DO_NOTHING,
            "倒した値が電源プランへ反映されている"
        );
        restored.expect("元の値へ戻せる");
        assert_eq!(
            imp::read(&scheme, Rail::Ac).expect("読める"),
            original,
            "元の値に戻っている"
        );
    }

    /// 記録の保存 → 読み出し → 破棄が、キャッシュ越しでも一致すること（#697）。
    ///
    /// 置き場所は引数で渡す（`TAKO_DATA_DIR` を差し替えない）。環境変数も `CACHE` も
    /// プロセス共有なので、`cargo test` の並列実行下で片方だけ差し替えると
    /// 同一バイナリの他テストを巻き込む（言語グローバルで実害を出した #608 と同型）。
    /// キャッシュだけは共有のままなので、機械の状態を触るテストと錠を共有して直列化する
    #[test]
    fn 記録の保存と破棄がキャッシュへ反映される() {
        let _serial = crate::platform::testing::machine_state_lock();
        let scratch = tako_core::test_residue::ScratchDir::new("tako-lid-cache");
        let path = scratch.path().join("lid-guard.json");
        invalidate_cache();

        let state = SavedLidState {
            method: LidMethod::PowerPlan,
            scheme: "381b4222-f694-41f0-9685-ff5bb260df2e".to_string(),
            ac: Some(1),
            dc: None,
            owner: Some(RecordOwner::current()),
        };
        store_saved_at(&path, &state).expect("保存できる");
        assert_eq!(
            load_saved_from(Some(&path)),
            Ok(Some(state.clone())),
            "書いた記録が読める"
        );

        // ディスクを直接読んでも同じ（キャッシュだけに入って消えていない）
        invalidate_cache();
        assert_eq!(
            load_saved_from(Some(&path)),
            Ok(Some(state)),
            "再読み込みでも同じ"
        );

        clear_saved_at(Some(&path));
        assert_eq!(
            load_saved_from(Some(&path)),
            Ok(None),
            "破棄でキャッシュも空になる"
        );
        invalidate_cache();
        assert_eq!(
            load_saved_from(Some(&path)),
            Ok(None),
            "ディスクからも消えている"
        );

        // 次のテストへ我々の写しを持ち越さない
        invalidate_cache();
    }

    /// 置き場所が解決できない環境（データディレクトリ無し）でも壊れない
    #[test]
    fn 置き場所が無ければ記録は空として扱う() {
        let _serial = crate::platform::testing::machine_state_lock();
        invalidate_cache();
        assert_eq!(load_saved_from(None), Ok(None));
        clear_saved_at(None); // remove_file を呼ばずに写しだけ空にする
        assert_eq!(load_saved_from(None), Ok(None));
        invalidate_cache();
    }

    /// GUID の往復が実機の値でも壊れないこと（記録の読み書きが成立する前提）
    #[cfg(windows)]
    #[test]
    fn 実機のプランguidが往復する() {
        let scheme = imp::active_scheme().expect("取れる");
        let text = imp::guid_to_string(&scheme);
        let back = imp::guid_from_string(&text).expect("解釈できる");
        assert_eq!(imp::guid_to_string(&back), text);
    }
}

//! 蓋を閉じたまま走らせ続ける制御（抽象境界 B9 のうち蓋ぶん）の **macOS 以外**の実装。#697
//!
//! macOS は clamshell 検知 + sudoers + `pmset disablesleep` を `sleep_guard` が持っていて
//! そちらが正。このモジュールは非 macOS 経路の差し込み口で、`cfg` はこのファイルの内側に閉じている。
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

/// 倒す前に保存しておく元の状態。クラッシュしてもここから戻せる
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedLidState {
    /// 倒した電源プランの GUID（文字列）。
    /// ユーザーがプランを切り替えても、書いたプランへ戻せるように持つ
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
    if let Some(secs) = tako_core::platform::procinfo::start_time_unix(pid) {
        return Owner::Alive {
            started: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
        };
    }
    // 起動時刻が取れない = 居ない、または読めない。生きているなら起動時刻なしの
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

/// 記録の読み取り結果。`Err` = 解釈できない。
/// **「記録なし」へ丸めない**（丸めると #1373 の症状 2 が起きる）
type RecordRead = Result<Option<SavedLidState>, String>;

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
        Err(e) => return Err(format!("記録を読めません（{}）: {e}", path.display())),
    };
    match serde_json::from_str::<SavedLidState>(&text) {
        Ok(state) => Ok(Some(state)),
        Err(e) => {
            let quarantine =
                tako_core::migration::quarantine_unreadable(path, &tako_core::migration::FsIo);
            let where_to = match quarantine {
                Some(dest) => format!("。内容は {} へ退避しました", dest.display()),
                None => String::new(),
            };
            Err(format!(
                "蓋閉じ継続の記録を解釈できません（{}）: {e}{where_to}",
                path.display()
            ))
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
    f: impl FnOnce(Option<SavedLidState>) -> Result<T, String>,
) -> Result<T, String> {
    let _lock = crate::config_io::lock_exclusive(path)?;
    let fresh = read_from(path);
    *lock_cache() = Some(fresh.clone());
    f(fresh?)
}

fn store_saved(state: &SavedLidState) -> Result<(), String> {
    let path = state_path().ok_or_else(|| "データディレクトリが解決できません".to_string())?;
    store_saved_at(&path, state)
}

/// 記録を書く。**原子書き込み**（tmp + fsync + rename）を通すので、並行プロセスの
/// 読み手には旧内容か新内容しか見えない（#169 の窓をここにも作らない = #1373 の症状 2）
fn store_saved_at(path: &std::path::Path, state: &SavedLidState) -> Result<(), String> {
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    crate::config_io::atomic_write(path, &text)?;
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

/// テスト用: キャッシュを捨てて次回ディスクから読み直させる
#[cfg(test)]
fn invalidate_cache() {
    *lock_cache() = None;
}

/// この OS で蓋閉じ継続を制御できるか
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
    let path = state_path().ok_or_else(|| "データディレクトリが解決できません".to_string())?;
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
) -> Result<bool, String> {
    match action {
        LidAction::Nothing => Ok(false),
        LidAction::Restore => {
            let saved = saved.ok_or_else(|| "戻す記録がありません".to_string())?;
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
    let path = state_path().ok_or_else(|| "データディレクトリが解決できません".to_string())?;
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
fn acquire(rails: &[Rail]) -> Result<(), String> {
    let scheme = imp::active_scheme()?;
    let mut state = SavedLidState {
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
fn restore(saved: &SavedLidState) -> Result<(), String> {
    let scheme = imp::guid_from_string(&saved.scheme)
        .ok_or_else(|| format!("記録の GUID を解釈できません: {}", saved.scheme))?;
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

#[cfg(windows)]
mod imp {
    use super::Rail;
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
    pub(super) fn active_scheme() -> Result<Guid, String> {
        let mut ptr: *mut Guid = std::ptr::null_mut();
        // SAFETY: 出力先はスタック上のポインタ。成功時のみ中身を読む
        let rc = unsafe { PowerGetActiveScheme(std::ptr::null_mut(), &mut ptr) };
        if rc != ERROR_SUCCESS || ptr.is_null() {
            return Err(format!("PowerGetActiveScheme に失敗（rc={rc}）"));
        }
        // SAFETY: rc が成功 かつ 非 NULL。API 仕様どおり LocalFree で解放する
        let guid = unsafe { *ptr };
        // SAFETY: PowerGetActiveScheme が確保したバッファ。解放は LocalFree が正
        unsafe { LocalFree(ptr as *mut c_void) };
        Ok(guid)
    }

    pub(super) fn read(scheme: &Guid, rail: Rail) -> Result<u32, String> {
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
            return Err(format!("蓋の設定を読めません（{rail:?}, rc={rc}）"));
        }
        Ok(value)
    }

    pub(super) fn write(scheme: &Guid, rail: Rail, value: u32) -> Result<(), String> {
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
            return Err(format!("蓋の設定を書けません（{rail:?}, rc={rc}）"));
        }
        Ok(())
    }

    /// 書いた値を有効化する。**これを呼ばないと反映されない**
    pub(super) fn apply(scheme: &Guid) -> Result<(), String> {
        // SAFETY: scheme は生存する参照
        let rc = unsafe { PowerSetActiveScheme(std::ptr::null_mut(), scheme) };
        if rc != ERROR_SUCCESS {
            return Err(format!("電源プランを適用できません（rc={rc}）"));
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
    use super::Rail;

    /// macOS は `sleep_guard` の clamshell + pmset 実装が担当するのでここへは来ない。
    /// Linux 等は蓋の動作を触る共通の仕組みが無い
    pub(super) const SUPPORTED: bool = false;

    /// 非 Windows では GUID を扱わないので、型だけ合わせた空実装。
    ///
    /// `()` の別名にすると呼び出し側の `let scheme = active_scheme()?` が
    /// 「unit を束縛している」と clippy に叱られる（`let_unit_value`）。
    /// **境界の都合を呼び出し側の `allow` へ漏らさない**ために、
    /// ここでサイズゼロの専用型を持つ
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct Guid;

    pub(super) fn active_scheme() -> Result<Guid, String> {
        Err("この OS では蓋の設定を扱えません".to_string())
    }
    pub(super) fn read(_scheme: &Guid, _rail: Rail) -> Result<u32, String> {
        Err("この OS では蓋の設定を扱えません".to_string())
    }
    pub(super) fn write(_scheme: &Guid, _rail: Rail, _value: u32) -> Result<(), String> {
        Err("この OS では蓋の設定を扱えません".to_string())
    }
    pub(super) fn apply(_scheme: &Guid) -> Result<(), String> {
        Err("この OS では蓋の設定を扱えません".to_string())
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
    /// 自分の pid は必ず生きていて起動時刻が取れる。看取った pid は `Dead`
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

        // 看取った pid = 確実に死んでいる（大きい適当な数は再利用中の生者に当たりうる）
        let scratch = tako_core::test_residue::ScratchDir::new("tako-lid-reap");
        let dead_pid = reaped_pid(scratch.path());
        assert_eq!(probe_owner(dead_pid), Owner::Dead);
        let rec = record_owned_by(Some(owner(dead_pid, Some(1_700_000_000))));
        let claim = claim_for(rec.owner.as_ref(), me(), probe_owner);
        assert_eq!(claim, RecordClaim::Abandoned);
        assert_eq!(
            decide(false, rails_for(false), Some((&rec, claim))),
            LidAction::Restore,
            "死んだ所有者の残留は実 probe でも回収する"
        );
    }

    /// 「確実に死んでいる pid」を作る（起こして看取る）。
    /// 子はこのテストバイナリ自身で、**どのテストにも一致しないフィルタ**を渡すので
    /// 0 件走って即終了する。一時ファイルの置き場は使い捨てへ向ける（#1296 / #1312）
    fn reaped_pid(scratch: &std::path::Path) -> u32 {
        let exe = std::env::current_exe().expect("テストバイナリのパス");
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["--exact", "__tako_lid_no_such_test__", "--test-threads=1"])
            .env("TMPDIR", scratch)
            .env("TMP", scratch)
            .env("TEMP", scratch)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Windows でもコンソール窓を出さない（#628 / #586 の門を通す）
        let mut child = tako_core::platform::process::no_console_window(&mut cmd)
            .spawn()
            .expect("看取り用の子を起こせる");
        let pid = child.id();
        child.wait().expect("子を看取れる");
        assert!(
            !tako_core::platform::process::pid_alive(pid),
            "看取った pid {pid} が生きている"
        );
        pid
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
            assert!(
                err.contains("解釈できません"),
                "理由が出ていない（{tag}）: {err}"
            );
            let quarantine = tako_core::migration::quarantine_path(&path);
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
            scheme: "x".to_string(),
            ac: Some(1),
            dc: None,
            owner: None,
        };
        let both = SavedLidState {
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

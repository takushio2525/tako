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

/// 記録のメモリ上の写し。`None` = まだディスクから読んでいない。
///
/// **毎 tick のディスク読みを避けるため**にキャッシュする。`update()` は 2 秒ごとに
/// UI スレッドから呼ばれるので、ここで無条件にファイルを読むと
/// #212（pmset）・#168（claude agents）と同じ「UI スレッドの定期 I/O」を作ってしまう。
/// 記録を書き換えるのはこのプロセスだけなので、一度読んだら以後は写しが正
#[allow(clippy::option_option)]
static CACHE: Mutex<Option<Option<SavedLidState>>> = Mutex::new(None);

fn lock_cache() -> std::sync::MutexGuard<'static, Option<Option<SavedLidState>>> {
    match CACHE.lock() {
        Ok(g) => g,
        // 毒されていても蓋の制御は続けたい（残留を放置する方が害が大きい）
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn read_from_disk() -> Option<SavedLidState> {
    let path = state_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 記録を取り出す（初回だけディスクを読む）
fn load_saved() -> Option<SavedLidState> {
    let mut cache = lock_cache();
    if cache.is_none() {
        *cache = Some(read_from_disk());
    }
    cache.as_ref().and_then(|v| v.clone())
}

fn store_saved(state: &SavedLidState) -> Result<(), String> {
    let path = state_path().ok_or_else(|| "データディレクトリが解決できません".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    *lock_cache() = Some(Some(state.clone()));
    Ok(())
}

fn clear_saved() {
    if let Some(path) = state_path() {
        let _ = std::fs::remove_file(path);
    }
    *lock_cache() = Some(None);
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
/// `status()` / `update()` から毎 tick 引かれるので、記録の複製を作らずに真偽だけ見る
pub fn is_active() -> bool {
    if !supported() {
        return false;
    }
    let mut cache = lock_cache();
    if cache.is_none() {
        *cache = Some(read_from_disk());
    }
    cache.as_ref().is_some_and(|v| v.is_some())
}

/// 蓋閉じ時の動作を倒す / 元へ戻す。
///
/// - `enable = true`: `include_battery` が示すレールを「何もしない」へ倒す
/// - `enable = false`: 記録してある元値へ戻す
///
/// 同じ状態への再要求は何もしない（毎 tick 呼ばれる前提）
pub fn set_stay_awake(enable: bool, include_battery: bool) -> Result<bool, String> {
    if !supported() {
        return Ok(false);
    }
    let saved = load_saved();
    if enable {
        let wanted = rails_for(include_battery);
        if let Some(ref cur) = saved {
            // 既に目的のレールと過不足なく一致しているなら何もしない
            if cur.covers_exactly(wanted) {
                return Ok(false);
            }
            // 電源条件が変わった（ac-only ⇔ always）。いったん全部戻してから倒し直す
            restore(cur)?;
        }
        acquire(wanted).map(|_| true)
    } else {
        match saved {
            Some(ref cur) => restore(cur).map(|_| true),
            None => Ok(false),
        }
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
    let saved = load_saved();
    if let Err(_reason) =
        should_clear_residual(is_isolated, other_instance_running, saved.is_some())
    {
        return Ok(None);
    }
    let saved = saved.expect("should_clear_residual が saved_exists を検査済み");
    restore(&saved)?;
    Ok(Some(format!(
        "蓋閉じ継続の上書きを解除しました（前回のクラッシュまたは異常終了）: scheme={}",
        saved.scheme
    )))
}

/// 倒す。元値を**保存してから**書く（保存前に落ちても残留しない順序）
fn acquire(rails: &[Rail]) -> Result<(), String> {
    let scheme = imp::active_scheme()?;
    let mut state = SavedLidState {
        scheme: imp::guid_to_string(&scheme),
        ac: None,
        dc: None,
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

/// 元値へ戻す。ユーザーが変えていたレールは触らない
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

    /// 非 Windows では GUID を扱わないので、型だけ合わせた空実装
    pub(super) type Guid = ();

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

/// `PowerGetActiveScheme` が返す GUID を `powercfg` と同じ表記へ
fn guid_fmt(data1: u32, data2: u16, data3: u16, data4: &[u8; 8]) -> String {
    format!(
        "{data1:08x}-{data2:04x}-{data3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        data4[0], data4[1], data4[2], data4[3], data4[4], data4[5], data4[6], data4[7]
    )
}

/// `guid_fmt` の逆。壊れた記録は `None`（呼び出し側は残留復元を諦める）
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
        };
        let both = SavedLidState {
            scheme: "x".to_string(),
            ac: Some(1),
            dc: Some(1),
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
        };
        assert_eq!(state.entries(), vec![(Rail::Ac, 1), (Rail::Dc, 2)]);
        assert!(state.covers(Rail::Ac) && state.covers(Rail::Dc));
    }

    /// 非対応 OS では倒す操作が無害に素通りする（macOS CI で常に通る）
    #[test]
    fn 非対応osでは素通りする() {
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
    /// `TAKO_DATA_DIR` を差し替えるので**本番の記録には触らない**。
    /// 環境変数はプロセス共有なので、この 1 本だけで完結させる（他テストと混ぜない）
    #[test]
    fn 記録の保存と破棄がキャッシュへ反映される() {
        let dir = std::env::temp_dir().join(format!("tako-lid-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        // SAFETY: このテストは環境変数を触る唯一のテストで、後始末まで自分で行う
        unsafe { std::env::set_var("TAKO_DATA_DIR", &dir) };
        invalidate_cache();

        let state = SavedLidState {
            scheme: "381b4222-f694-41f0-9685-ff5bb260df2e".to_string(),
            ac: Some(1),
            dc: None,
        };
        store_saved(&state).expect("保存できる");
        assert_eq!(load_saved().as_ref(), Some(&state), "書いた記録が読める");

        // ディスクを直接読んでも同じ（キャッシュだけに入って消えていない）
        invalidate_cache();
        assert_eq!(load_saved().as_ref(), Some(&state), "再読み込みでも同じ");

        clear_saved();
        assert_eq!(load_saved(), None, "破棄でキャッシュも空になる");
        invalidate_cache();
        assert_eq!(load_saved(), None, "ディスクからも消えている");

        // SAFETY: 上で設定した変数を戻す
        unsafe { std::env::remove_var("TAKO_DATA_DIR") };
        invalidate_cache();
        let _ = std::fs::remove_dir_all(&dir);
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
